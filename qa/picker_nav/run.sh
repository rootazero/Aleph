#!/usr/bin/env bash
# Real-machine QA fixture for the keyboard walk + the conditional bottom fade
# + the phone add-a-provider flow.
#
#   ./qa/picker_nav/run.sh          # boot an isolated server, print the checklist
#   KEEP=1 ./qa/picker_nav/run.sh   # keep the scratch dir for post-mortem
#
# BOOTS AND WAITS. Every item is Panel-side interaction, so there is no mock
# provider and no agent turn here — what the fixture supplies is a realistic
# catalogue (two presets configured, fifty-odd not) and three widths.
#
# The three widths are the point of the round's QA, not a formality. The
# desktop settings master-detail folds at `@media (max-width: 720px)`
# (`.aleph-md` in styles/tailwind.css), while the *form factor* switch to the
# phone UI is at 640px — so 641–720px is a band that renders the desktop
# screens in their stacked form, and the previous round tested neither it nor
# the phone UI.
#
# Same scratch-HOME discipline as qa/canvas/run.sh: build BEFORE $HOME is
# redirected (cargo's registry lives under the real HOME), then run with HOME
# *and* ALEPH_HOME inside a throwaway root.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
QA_ROOT="${QA_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-picker-XXXXXX")}"
KEEP="${KEEP:-0}"
GATEWAY_PORT="${GATEWAY_PORT:-18797}"

. "$HERE/../lib/scratch_home.sh"
qa_redirect_home "$QA_ROOT"
mkdir -p "$ALEPH_HOME"
CONFIG="$ALEPH_HOME/config.toml"

export RUST_MIN_STACK="${RUST_MIN_STACK:-268435456}"

TARGET_DIR="$(cd "$REPO" && HOME="$REAL_HOME" cargo metadata --no-deps --format-version 1 2>/dev/null \
  | python3 -c 'import sys,json; print(json.load(sys.stdin)["target_directory"])' 2>/dev/null)"
BIN="${TARGET_DIR:-$REPO/target}/debug/aleph-server"
SERVER_PID=""

say() { printf '\n=== %s ===\n' "$*"; }

cleanup() {
  [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null
  sleep 1
  [ -n "$SERVER_PID" ] && kill -9 "$SERVER_PID" 2>/dev/null
  if [ "$KEEP" = "1" ]; then echo "artifacts kept in $QA_ROOT"; else rm -rf "$QA_ROOT"; fi
}
trap cleanup EXIT

say "build"
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  if ! (cd "$REPO" && HOME="$REAL_HOME" cargo build --bin aleph-server 2>&1 | tail -5); then
    echo "build failed" >&2; exit 1
  fi
fi
[ -x "$BIN" ] || { echo "no binary at $BIN" >&2; exit 1; }
# Debug servers read the Panel's dist/ from disk (rust_embed debug mode); an
# empty dist serves a blank page and every item "fails" for the wrong reason.
if [ ! -f "$REPO/interfaces/webchat/dist/index.html" ]; then
  echo "interfaces/webchat/dist/ has no build — run \`just wasm\` first" >&2
  exit 69
fi

say "generate a baseline config"
timeout 25 "$BIN" start >"$QA_ROOT/gen.log" 2>&1 &
GEN_PID=$!
for _ in $(seq 1 50); do [ -f "$CONFIG" ] && break; sleep 0.5; done
kill "$GEN_PID" 2>/dev/null; wait "$GEN_PID" 2>/dev/null
[ -f "$CONFIG" ] || { echo "no config generated at $CONFIG"; tail -20 "$QA_ROOT/gen.log"; exit 1; }

say "patch config (inert daemon, two configured catalogue presets)"
python3 "$HERE/patch_config.py" "$CONFIG" --gateway-port "$GATEWAY_PORT" || exit 1

say "start server"
"$BIN" start >"$QA_ROOT/server.log" 2>&1 &
SERVER_PID=$!
for _ in $(seq 1 90); do
  if curl -sf -o /dev/null "http://127.0.0.1:$GATEWAY_PORT/health" 2>/dev/null; then break; fi
  kill -0 "$SERVER_PID" 2>/dev/null || { echo "server died"; tail -40 "$QA_ROOT/server.log"; exit 1; }
  sleep 1
done
echo "gateway up on $GATEWAY_PORT"

say "manual checklist (chrome-devtools-mcp, 每条带效果断言)"
cat <<'CHECKLIST'
  wide (1440x900) — /settings/providers
   1. 展开「添加提供商」→ 列表底部有渐隐；滚到底渐隐消失；搜索到 <6 行时渐隐消失
      断言: 容器 classList 含/不含 `aleph-scroll-more`，与 scrollHeight-scrollTop-clientHeight>1 一致
   2. ↓ 按 30 次（远超行数）后按一次 ↑ → 高亮立刻上移一行
      断言: 高亮行 index 从 len-1 变成 len-2（旧版要按 30 次 ↑ 才动）
   3. ↓ 越过可视区 → 该行自动滚入视野，且外层设置面板不跟着跳
      断言: 高亮行 offsetTop 在容器可视区内；容器外层 scrollTop 不变

  wide — ⌘K 命令面板
   4. 同 2：↓ ×30 后 ↑ 一次立刻上移
      断言: `.bg-primary\/12` 的那一行从最后一行变成倒数第二行
   5. 面板行数超过 50vh 时底部有渐隐，滚到底消失

  wide — 聊天窗 model picker
   6. 打开 pill → 搜索框自动获得焦点；↑/↓ 走 [Default]+每个模型；Enter 选中
      断言: 选完 pill 文案变成 provider/model；Esc 关闭

  narrow fold (700x900) — 上一轮没测的那一档
   7. /settings/providers 变成上下堆叠（左右分栏消失），页面整体滚动
      断言: `.aleph-md` 的 flex-direction === 'column'
   8. 该形态下 1/2/3 全部仍成立（渐隐 + ↑/↓ + 滚入视野）

  phone (390x844) — /settings/providers（iOS 形态）
   9. 「添加提供商」cell 展开 → 搜索 → 选一个未配置的预设 → 填模型 + 密钥 → 添加
      断言: 列表出现该 provider 且展开；config.toml 里多出 [providers.<id>]
  10. 选一个已配置的预设（groq / deepseek，带「已配置」角标）
      断言: 不进入设置表单，直接关闭 picker 并展开那一行现有的编辑区
  11. 长列表底部渐隐同 1
CHECKLIST

cat <<EOF

  Panel:    http://127.0.0.1:$GATEWAY_PORT
  scratch:  $QA_ROOT   (config: $CONFIG)
  logs:     $QA_ROOT/server.log · \$ALEPH_HOME/logs/
  item 9 的落盘断言:  grep -A3 '^\[providers\.' $CONFIG

  Server stays up until Ctrl-C (scratch root removed on exit; KEEP=1 keeps it).
EOF

wait "$SERVER_PID"
