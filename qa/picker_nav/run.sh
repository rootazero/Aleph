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
# Items 1-13 and 16-18 run as loopback operator, which the fixture boots into
# directly. Items 14-15 cannot: loopback is *always* operator, so the refusal
# half of these screens — the only path that reaches
# `admin_refusal::settings_write_error`, which is what this round's new
# Test/Delete buttons are wired through — needs a LAN bind, TLS and a member
# credential. `member_seed.py` turns the seeding half of that into one command;
# the browser half stays manual, which is the point of a real-machine QA.
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

  phone (390x844) — 本轮新增的两个按钮（loopback = operator）
  12. 展开一个已配置 provider → 点「测试连接」
      断言: 标题变「测试中...」→ 右侧出现绿色「连接成功」或红色「连接失败」；
            成功后重新加载，config.toml 里该 provider 的 verified = true
            （verified 的唯一写者就是 providers.test —— 手机端此前无路可写）
      断言(串味): 折叠该行、展开另一个 provider → 新行右侧**没有**上一行的判定
                  （判定按 provider 名字键控，不是一个裸 bool）
  13. 同一行点「删除提供商」→ 出现红色「确认删除？」+ 说明 + 「取消」两格
      断言: 点「取消」回到单格；点「确认删除？」后该行消失、
            config.toml 里 [providers.<id>] 段消失

  member（需要 LAN + TLS，见下方 member 段）— 本轮此前从未真机跑过的那一半
  14. member 打开 /settings/providers（phone）
      断言 a: 顶部是**被分类过的**那句话（settings.admin_refusal.read_config,
            「该设置页需要 operator 权限…」），不是裸协议串
      断言 b: **没有**「暂无配置的 Provider」。2026-08-18 首跑时它和 a 同屏出现,
            而 seeder 的 operator_sees_providers 是 ["deepseek","groq"] ——
            一个正确的拒绝横幅底下压着一句自信的假话,正是「被拒不许读作没有」。
            已修: 空状态改为只在 list_loaded（只有 Ok 会置位）时才敢断言
      断言 c: 展开「添加提供商」后显示「未能读取提供商目录。」,
            **不是**「没有匹配的提供商」——目录不是空的,是被拒了
      对照(operator, loopback): 同一块代码在真为空时必须仍说「没有匹配的提供商」。
            少了这一半,修复就可能是把一句谎话换成另一句
  15. ⚠️ **member 在这个屏幕上结构性到不了写路径**（2026-08-18 实测结论,
      不是没跑）: providers.list 与 providers.catalog 都被 ADMIN_PREFIXES 拒掉,
      于是没有任何 provider 行、也没有任何目录行可以作用 ——「测试连接」/
      「删除提供商」/「保存」三个按钮一个都渲染不出来。
      要覆盖 settings_write_error 那条臂,得换一个 member 读得到、写不了的
      surface（本轮未做）

  i18n（本轮把 phone 平台 53 处硬编码中文换成 t!）
  16. 桌面「设置 → 通用」把语言切到 English，再回到 phone 宽度
      断言: /settings/providers · /settings/embeddings · /settings/model-route ·
            /settings/appearance · /settings/connection · /memory 五屏
            全英文；不残留任何中文
      断言(实时): 不刷新页面切换语言，列表分组标题（Theme/Material/…）当场变
            （这些标题是 Signal<String>，快照式 &'static str 会卡在旧语言）

  通道设置（本轮切掉 MS Teams 卡片、给飞书补了工厂）
  17. wide /settings/channels
      断言: 网格里**没有** Microsoft Teams 卡片；**有** Feishu / Lark 卡片
      断言: 也有 LINE / WeChat / QQ 三张卡（2026-08-18 补齐）。补齐它们不是
            "顺便加功能"——卡片集合从此**等于** CONFIGURABLE_CHANNEL_TYPES，
            于是那条对账断言两个方向都是集合相等，不需要任何豁免清单
      断言: 打开 QQ 卡片填 App ID + Client Secret 保存 → config.toml 里
            [channels.qq] 是**扁平**的（没有 accounts 数组）。服务端由
            QQConfig::from_wire 归一化；qa/channels/run.sh 在真实开机路径上验它
  18. → 已搬走：`./qa/channels/run.sh`
      这一项从 2026-08-18 起是一个**可执行 fixture**，不再是人工清单条目。
      它跑 16 条断言：feishu / line / qq 各自被工厂构造；msteams 作为对照组
      被 resolved_channels() 丢弃**并出声**；feishu 的 start() 对着本地
      mock Lark 真拨号（取 token → 取 bot info → 起 webhook server）；
      一条签名事件进 webhook → agent 回合 → 回复从真正的 Feishu 发送路径
      打回 mock 的 im/v1/messages，收件人等于事件来源的 chat。
      搬走的理由就是这一项自己的历史：它作为一段要人读、要人照做的散文，
      **第一版断言写错了**（去找一条这条路径根本不会打印的
      `Failed to create channel`），而错了没有任何东西会告诉你。

CHECKLIST

cat <<EOF

  Panel:    http://127.0.0.1:$GATEWAY_PORT
  scratch:  $QA_ROOT   (config: $CONFIG)
  logs:     $QA_ROOT/server.log · \$ALEPH_HOME/logs/
  item 9/12/13 的落盘断言:  grep -A6 '^\[providers\.' $CONFIG
  item 18:                  ./qa/channels/run.sh   (自带断言, 退出码=失败条数)

  member 段 (items 14-15) — 需要 LAN + TLS，因为 loopback 恒为 operator:
    1) 停掉 server, 编辑 $CONFIG:
         [gateway] host = "0.0.0.0"      [gateway.tls] enabled = true
    2) 重启 server, 然后对着 **loopback** 跑 seeder (它需要 operator):
         python3 $HERE/member_seed.py $GATEWAY_PORT --tls
    3) 从本机 LAN IP（不是 127.0.0.1）打开它打印的 member URL, TOFU 接受自签证书
    4) seeder 打印的 operator_sees_providers 就是 item 14 的对照组 ——
       它非空而 member 屏幕说「暂无配置的 Provider」, 即为缺陷

  Server stays up until Ctrl-C (scratch root removed on exit; KEEP=1 keeps it).
EOF

wait "$SERVER_PID"
