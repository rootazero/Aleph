#!/usr/bin/env bash
# Real-machine QA fixture for the whiteboard canvas (plan Task 20).
#
#   ./qa/canvas/run.sh          # boot an isolated server, print the checklist,
#                               # stay in the foreground until Ctrl-C
#   KEEP=1 ./qa/canvas/run.sh   # keep the scratch dir for post-mortem
#
# This fixture BOOTS AND WAITS — the nine checklist items below are driven by
# hand (Panel in a browser, chrome-devtools-mcp, or both), because every one
# of them is about live UI behaviour: broadcast latency, conflict recovery,
# fullscreen presentation. Each item carries its own effect assertion.
#
# Same scratch-HOME discipline as qa/busy_input/run.sh: build happens BEFORE
# $HOME is redirected (cargo's registry lives under the real HOME), then the
# server runs with HOME *and* ALEPH_HOME inside a throwaway root, so nothing
# in the run can read or write the operator's real ~/.aleph — including the
# secrets vault: the mock-provider api_key is INLINE in the generated config
# (`ProviderConfig.api_key` is skip_serializing but still deserializes).
#
# Idempotent: every invocation mints a fresh scratch root and tears it down
# on exit (KEEP=1 to keep). Ports are env-overridable for parallel runs.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
SHARED="$HERE/../busy_input"
QA_ROOT="${QA_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-canvas-XXXXXX")}"
KEEP="${KEEP:-0}"

GATEWAY_PORT="${GATEWAY_PORT:-18798}"
MOCK_PORT="${MOCK_PORT:-18994}"
# Optional: a mock tool-spec JSON ({"name": ..., "input": {...}}) — the mock
# emits exactly this tool call on every tool turn. Checklist items 4–6 use it
# once the live canvas/frame ids are known; see README.md for the recipe.
MOCK_TOOL_SPEC="${MOCK_TOOL_SPEC:-}"

# Build BEFORE HOME is redirected — cargo's registry, git cache and rustup
# toolchain all live under the real HOME. See qa/README.md.
. "$HERE/../lib/scratch_home.sh"
. "$HERE/../lib/build.sh"
# Redirects HOME/ALEPH_HOME into the scratch root AND pins RUSTUP_HOME/
# CARGO_HOME at the real ones — the redirect and the pin are inseparable
# on purpose; see that file for the 1.3 GB-per-run leak it closes.
qa_redirect_home "$QA_ROOT"
mkdir -p "$ALEPH_HOME"
CONFIG="$ALEPH_HOME/config.toml"

export RUST_MIN_STACK="${RUST_MIN_STACK:-268435456}"

# NOT `$REPO/target`: this repo resolves a shared target directory, so a git
# worktree builds into the MAIN checkout's target tree and `$REPO/target` does
# not exist at all. Ask cargo instead of assuming — with the real HOME, since
# `cargo metadata` reads the registry.
TARGET_DIR="$(cd "$REPO" && HOME="$REAL_HOME" cargo metadata --no-deps --format-version 1 2>/dev/null \
  | python3 -c 'import sys,json; print(json.load(sys.stdin)["target_directory"])' 2>/dev/null)"
BIN="${TARGET_DIR:-$REPO/target}/debug/aleph-server"
MOCK_PID=""
SERVER_PID=""

say() { printf '\n=== %s ===\n' "$*"; }

cleanup() {
  [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null
  [ -n "$MOCK_PID" ] && kill "$MOCK_PID" 2>/dev/null
  sleep 1
  [ -n "$SERVER_PID" ] && kill -9 "$SERVER_PID" 2>/dev/null
  [ -n "$MOCK_PID" ] && kill -9 "$MOCK_PID" 2>/dev/null
  if [ "$KEEP" = "1" ]; then
    echo "artifacts kept in $QA_ROOT"
  else
    rm -rf "$QA_ROOT"
  fi
}
trap cleanup EXIT

say "build"
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  if ! qa_build --bin aleph-server; then
    echo "build failed" >&2; exit 1
  fi
fi
[ -x "$BIN" ] || { echo "no binary at $BIN" >&2; exit 1; }
# The Panel is what drives every checklist item, and debug servers read its
# dist/ from disk (rust_embed debug mode) — an empty dist serves a blank page
# and every item "fails" for the wrong reason. Say so up front.
if [ ! -f "$REPO/interfaces/webchat/dist/index.html" ]; then
  echo "interfaces/webchat/dist/ has no build — run \`just wasm\` first" >&2
  exit 69
fi

say "generate a baseline config"
# `--port` on the GENERATION boot. The config does not exist yet, so without
# it this boot binds the built-in default port — and if anything already holds
# that port (another fixture, a dev server, the operator's own daemon) the
# process exits before writing a config at all. The symptom is
# `no config generated at …`, which reads like a permissions or path problem;
# the cause is one line further up the log. Binding the port this run already
# owns makes the generation boot as isolated as the real one.
timeout 25 "$BIN" --port "$GATEWAY_PORT" start >"$QA_ROOT/gen.log" 2>&1 &
GEN_PID=$!
for _ in $(seq 1 50); do [ -f "$CONFIG" ] && break; sleep 0.5; done
kill "$GEN_PID" 2>/dev/null; wait "$GEN_PID" 2>/dev/null
[ -f "$CONFIG" ] || { echo "no config generated at $CONFIG"; tail -20 "$QA_ROOT/gen.log"; exit 1; }

say "patch config (inert daemon, inline mock api_key — no vault)"
python3 "$SHARED/patch_config.py" "$CONFIG" \
  --gateway-port "$GATEWAY_PORT" --mock-port "$MOCK_PORT" || exit 1

say "start mock provider"
# `quick` plan: short turns, so a stray chat send never wedges the run slot.
# The request_log (5th arg) is wired UNCONDITIONALLY: the one canvas anomaly
# this fixture ever produced (an in-run insert_image that never committed,
# spec §8) was unattributable precisely because the log was off — the oracle
# costs nothing and only exists when it is already running. The empty 4th
# positional is how "no tool spec" is spelled when a 5th follows.
python3 "$SHARED/mock_anthropic.py" "$MOCK_PORT" /etc/hostname quick \
  "${MOCK_TOOL_SPEC:-}" "$QA_ROOT/request_log.jsonl" >"$QA_ROOT/mock.log" 2>&1 &
MOCK_PID=$!
sleep 1

say "start server"
"$BIN" start >"$QA_ROOT/server.log" 2>&1 &
SERVER_PID=$!

for _ in $(seq 1 90); do
  if curl -sf -o /dev/null "http://127.0.0.1:$GATEWAY_PORT/health" 2>/dev/null; then break; fi
  kill -0 "$SERVER_PID" 2>/dev/null || { echo "server died"; tail -40 "$QA_ROOT/server.log"; exit 1; }
  sleep 1
done
echo "gateway up on $GATEWAY_PORT"

say "manual checklist (chrome-devtools-mcp 执行，每条带效果断言)"
cat <<'CHECKLIST'
  1. 建画布→画矩形/便签/画笔→刷新页面→内容还在（持久化）
  2. 双标签页：A 画一笔 B 实时出现；B 移动形状 A 实时跟随（广播）
  3. A/B 同时拖同一形状→一端收冲突→自动重拉不丢另一端改动（乐观锁）
  4. 对话里让模型 `canvas(action='create')` + `insert_html`→Panel 实时弹出新画布内容（工具面+事件面）
  5. AI 图片框全流程（mock provider 返回固定 data URL 图）→框被图替换
  6. 标注重生成：标注→提交→模型插新图于原图旁
  7. Slides：三帧组 deck→播放→翻页→Esc
  8. member 角色（0.0.0.0 + 自签 TLS + 局域网 IP，配方见 memory）看不到 operator 的私有画布；房间画布双方可见可编辑
  9. PNG 导出落文件且可打开
 10. 左栏画廊：标题列表 + 打开高亮 + 搜索过滤 + 两面重命名（行内 / 编辑器标题）
     + 冷加载先「加载中」后「还没有画布」（断言见 README「Item 10」）
CHECKLIST

cat <<EOF

  Panel:      http://127.0.0.1:$GATEWAY_PORT  →「画布」/ Canvas in the sidebar
  scratch:    $QA_ROOT   (config: $CONFIG)
  logs:       $QA_ROOT/server.log · $QA_ROOT/mock.log · $ALEPH_HOME/logs/
  oracle:     $QA_ROOT/request_log.jsonl — every request body the mock saw,
              one JSON object per line; tool_result blocks in it are the only
              ground truth for "did the model's canvas call commit" (spec §8).
  items 4–6:  the mock emits one fixed tool call per tool turn. Once the live
              canvas/frame ids exist, restart ONLY the mock with a tool spec
              (server keeps running, state survives — keep the request_log arg):
                kill $MOCK_PID
                python3 $SHARED/mock_anthropic.py $MOCK_PORT /etc/hostname quick /path/to/spec.json $QA_ROOT/request_log.jsonl
              spec recipe (item 5): see qa/canvas/README.md.
  item 8:     needs a LAN bind + member credentials — recipe in README.md;
              this scenario boots loopback-only on purpose.

  Server stays up until Ctrl-C (scratch root is removed on exit; KEEP=1 keeps it).
EOF

wait "$SERVER_PID"
