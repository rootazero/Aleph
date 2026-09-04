#!/usr/bin/env bash
# Real-machine QA fixture for the keyboard walk + the conditional bottom fade
# + the phone add-a-provider flow.
#
#   ./qa/picker_nav/run.sh          # boot an isolated server, print the checklist
#   KEEP=1 ./qa/picker_nav/run.sh   # keep the scratch dir for post-mortem
#
# BOOTS AND WAITS. Every item but 12 is Panel-side interaction, so there is no
# agent turn here — what the fixture supplies is a realistic catalogue (presets
# configured, fifty-odd not) and three widths.
#
# Item 12 is the exception and gets a stub: "test connection" is *entirely* a
# round-trip that leaves the process, so a Panel-only fixture cannot reach it
# at all. `mock_provider.py` answers the probe locally, which keeps the run
# offline and keyless while still exercising the real client stack; the failure
# arm is a provider aimed at a closed port, so that refusal is real too.
#
# Items 1-13 and 16-18 run as loopback operator, which the fixture boots into
# directly. Items 14-15 cannot: loopback is *always* operator, so the refusal
# half of these screens — the only path that reaches
# `admin_refusal::settings_write_error`, which is what this round's new
# Test/Delete buttons are wired through — needs a LAN bind, TLS and a member
# credential. `member_seed.py` turns the seeding half of that into one command;
# the browser half stays manual, which is the point of a real-machine QA.
#
# ## Instrument caveats (2026-08-18, cost real time — read before driving)
#
#   * Drive the phone items with chrome-devtools-mcp `emulate`
#     (`390x844x3,mobile,touch`), NOT a window resize. `resize_window` reports
#     success and leaves `innerWidth` unchanged (macOS enforces a window
#     minimum well above 390), so every phone item silently runs against the
#     WIDE layout and passes or fails for the wrong reason. `resize_page` gets
#     under the 640px breakpoint but stops around 500px; only viewport
#     emulation reaches 390 exactly.
#   * `fill(uid, "")` sets `.value` WITHOUT dispatching `input`, so the Leptos
#     filter keeps the previous list and the picker looks stuck on the old
#     query. That is the instrument, not the product — a real Backspace
#     re-filters correctly. Clear the box with keystrokes (or dispatch
#     `new Event('input')` yourself) before reporting a filter bug.
#   * A connected browser that reports `isLocal: true` may still be unable to
#     reach this machine's loopback at all (seen 2026-08-18: three navigations
#     to 127.0.0.1 / localhost / :18797/health all ERR_CONNECTION_REFUSED while
#     `curl` on the same port answered 200). CLAUDE.md's rule is "the giveaway
#     is the source IP in the server log"; its degenerate case is that NO
#     request arrives. Check the server log before believing the Panel is broken.
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
# The stub item 12 probes, and a port nothing listens on for the failure arm.
MOCK_PORT="${MOCK_PORT:-18798}"
DEAD_PORT="${DEAD_PORT:-18799}"

. "$HERE/../lib/scratch_home.sh"
. "$HERE/../lib/build.sh"
qa_redirect_home "$QA_ROOT"
mkdir -p "$ALEPH_HOME"
CONFIG="$ALEPH_HOME/config.toml"

export RUST_MIN_STACK="${RUST_MIN_STACK:-268435456}"

TARGET_DIR="$(cd "$REPO" && HOME="$REAL_HOME" cargo metadata --no-deps --format-version 1 2>/dev/null \
  | python3 -c 'import sys,json; print(json.load(sys.stdin)["target_directory"])' 2>/dev/null)"
BIN="${TARGET_DIR:-$REPO/target}/debug/aleph-server"
SERVER_PID=""
MOCK_PID=""

say() { printf '\n=== %s ===\n' "$*"; }

cleanup() {
  [ -n "$MOCK_PID" ] && kill "$MOCK_PID" 2>/dev/null
  [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null
  sleep 1
  [ -n "$SERVER_PID" ] && kill -9 "$SERVER_PID" 2>/dev/null
  [ -n "$MOCK_PID" ] && kill -9 "$MOCK_PID" 2>/dev/null
  if [ "$KEEP" = "1" ]; then echo "artifacts kept in $QA_ROOT"; else rm -rf "$QA_ROOT"; fi
}
trap cleanup EXIT

say "build"
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  if ! qa_build --bin aleph-server; then
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

say "patch config (inert daemon, configured presets + the two item-12 rows)"
python3 "$HERE/patch_config.py" "$CONFIG" --gateway-port "$GATEWAY_PORT" \
  --mock-port "$MOCK_PORT" --dead-port "$DEAD_PORT" || exit 1

# The failure arm needs $DEAD_PORT to stay closed. Checked in python rather
# than with `nc`, because a missing `nc` would make the guard answer "not
# occupied" for a reason that has nothing to do with the port — and this whole
# fixture exists to stop items passing for the wrong reason.
say "check the failure-arm port is closed"
if ! python3 -c "
import socket, sys
s = socket.socket()
s.settimeout(0.5)
sys.exit(1 if s.connect_ex(('127.0.0.1', $DEAD_PORT)) == 0 else 0)
"; then
  echo "DEAD_PORT $DEAD_PORT is occupied — item 12's failure arm would be testing the wrong refusal" >&2
  exit 1
fi

say "start mock provider (item 12 success arm)"
python3 "$HERE/mock_provider.py" "$MOCK_PORT" "$QA_ROOT/probes.jsonl" \
  >"$QA_ROOT/mock.log" 2>&1 &
MOCK_PID=$!

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
      ⚠️ **滚到底那一半在 chrome-devtools-mcp / claude-in-chrome 下测不了，而且它的
      失败方式是假阴性**（渐隐留着不走，读起来像 bug）。`publish_more_below` 在
      rAF 回调里量几何，而受控标签页的 `document.visibilityState` 是 `hidden`,
      浏览器就不跑 rAF 了。**先跑这个探针再下结论**，别靠猜:
          const fired = await new Promise(res => {
            let done = false;
            requestAnimationFrame(() => { done = true; res(true) });
            setTimeout(() => { if (!done) res(false) }, 1500);
          });
          ({ rafFires: fired, visibility: document.visibilityState })
      2026-08-18 实测 `{rafFires:false, visibility:"hidden"}` ⇒ 该断言无效，不是失败。
      第三条（搜索到 <6 行渐隐消失）**不经 rAF**：列表重渲染后 class 直接没了,
      同日实测 `.aleph-scroll-more` 数量 1 → 0，**PASS**。
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

  ⚠️ 下面两档宽度 2026-08-18 仍未跑成: `resize_window` 回报成功而视口纹丝不动
  （实测请求 1440x900 / 390x844 / 1500x950，`innerWidth` 恒 606）。这不是"忘了跑",
  是这条工具链改不了视口——要跑 6/7/8 得用真的手动窗口，或换一个能设 device
  metrics 的驱动。606 < 640 ⇒ 受控浏览器落在**手机形态**，所以 9-13 反而跑得成。

  narrow fold (700x900) — 上一轮没测的那一档
   7. /settings/providers 变成上下堆叠（左右分栏消失），页面整体滚动
      断言: `.aleph-md` 的 flex-direction === 'column'
   8. 该形态下 1/2/3 全部仍成立（渐隐 + ↑/↓ + 滚入视野）

  phone (390x844) — /settings/providers（iOS 形态）  [2026-08-18 全部真机 PASS]
   9. 「添加提供商」cell 展开 → 搜索 → 选一个未配置的预设 → 填模型 + 密钥 → 添加
      断言: 列表出现该 provider 且展开；config.toml 里多出 [providers.<id>]
      实测: 选 Mistral AI + 填密钥 → 行出现且**已展开**（设为默认/启用/API 密钥/
            保存/测试连接/删除提供商 六格齐），config.toml 落 [providers.mistral]
            （base_url 来自预设、protocol=openai、verified=false、api_key 不在
            config 里）。密钥进了保管库的旁证: 该行密钥框此后显示
            「••••••••（已设置，输入新值覆盖）」
  10. 选一个已配置的预设（groq / deepseek，带「已配置」角标）
      断言: 不进入设置表单，直接关闭 picker 并展开那一行现有的编辑区
      实测: 点 Groq（带「已配置」）→ 无「配置 Groq」表单、picker 关闭、
            groq 行展开、上一次展开的 mistral 行收起
  11. 长列表底部渐隐同 1
      实测: 三态与几何一致 — 全表 sh3637/ch388 → 有 .aleph-scroll-more；
            搜到 1 行 → 无（且无可滚容器）；滚到底 remainder=0.5 → 无

  phone (390x844) — 新增的两个按钮（loopback = operator）  [2026-08-18 真机 PASS]
  12. 展开 qa-mock → 点「测试连接」（它指向本地 stub，全程离线、无需密钥）
      ⚠️ stub 协议要点（2026-08-18 受控浏览器那次踩过）: `providers.test` 发的是
            真的一轮 chat completion（probe_provider 里的 "ping"），照原样点真实
            provider 会去拨它的外网 base_url。stub 必须按 `stream:true` 回 **SSE**、
            usage 走空 choices 尾块；回普通 JSON 会得到一条读起来像"连不上"的失败。
            mock_provider.py 两种形态都实现，就是为了不吃这个亏。
      断言: 标题变「测试中...」→ 右侧出现绿色「连接成功」
            成功后重新加载，config.toml 里 qa-mock 的 verified 从 **false 变 true**
            （verified 的唯一写者就是 providers.test —— 手机端此前无路可写。
             这一行刻意不预置 true，否则这条断言什么也没证明）
      断言(真的拨出去了): $QA_ROOT/probes.jsonl 多一行且 model == "qa-mock-model"
            （探针在 wire 上不带 provider 名字，只带那一行配置里的 model；
             所以"我点的是这一行"只有这个文件答得了，按钮自己的回执两种情况同形）
      失败臂: 展开 qa-dead（指向一个没人监听的端口）→ 点「测试连接」
            断言: 右侧出现红色「连接失败」；probes.jsonl **不增行**
      断言(串味): 在 qa-mock（成功）与 qa-dead（失败）之间折叠/展开来回走
            → 每行右侧只出现它自己的判定，绝不继承上一行的
            （两行判定相反是这条断言成立的前提；两个成功分不出陈旧与新鲜）
  13. 在 item 9 新建的那一行点「删除提供商」→ 出现红色「确认删除？」+ 说明 + 「取消」两格
      断言: 点「取消」回到单格；点「确认删除？」后该行消失、
            config.toml 里 [providers.<id>] 段消失
      （刻意删 item 9 建的那行而不是 qa-mock/qa-dead：那两行是 item 12 的夹具，
        删掉它们会让重跑 12 需要重启 fixture）

  12/13 实测 2026-08-18:
      12 ✓ 用 MutationObserver 在点击**之前**布好, 抓到 TESTING→OK 的过渡帧
           （本地往返太快, 事后轮询会漏掉「测试中」而把它读成"没出现过"）
         ✓ 「连接成功」颜色 oklch(0.60 0.15 142) / qa-dead「连接失败」颜色
           var(--color-danger, oklch(0.58 0.20 25))
         ✓ config.toml: qa-mock verified false→**true**, 而 qa-dead 仍 false
           —— 写的是那一行, 不是一次通盘保存
         ✓ probes.jsonl 恰好 1 行 model="qa-mock-model"; 失败臂**不增行**
           （拨的是死端口, 根本没到 stub）
         ✓ 串味两个方向都试了: qa-mock 成功后展开 qa-dead → 裸「测试连接」;
           qa-dead 失败后回到 qa-mock → 仍是裸「测试连接」
      13 ✓ 两格确认（红色「确认删除？」+「删除后该提供商的密钥与设置都会移除。」
           + 「取消」）→ 点「取消」回到单格 → 再点开并确认 → 行消失且
           config.toml 的 [providers.mistral] 段消失

  member（需要 LAN + TLS，见下方 member 段）— 本轮此前从未真机跑过的那一半
  14. member 打开 /settings/providers（phone）  [2026-08-18 真机 PASS，四条断言全过]
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
      实测 2026-08-18（0.0.0.0 + TLS + 一次性配对票, 从本机 LAN IP 打开,
      TOFU 接受自签证书）:
            a ✓「该设置页需要 operator 权限,当前连接的角色无法读取服务器全局配置。」
              全屏无任何裸协议串 / 错误码
            b ✓ 无「暂无配置的 Provider」, 而 seeder 同一时刻报
              operator_sees_providers = [groq, qa-dead, deepseek, qa-mock]
              —— 空状态有东西可撒谎却没撒
            c ✓「未能读取提供商目录。」, 且**搜索框根本没渲染**
            对照 ✓ operator 在 loopback 同一屏搜 "zzzzzz" → 仍说「没有匹配的提供商」
              两句话对应两个事实, 不是一句谎话换成另一句
      ⚠️ 跑完把 host 改回 127.0.0.1、TLS 关掉再收工 —— 这一段会把一个端口
         摆到局域网上, 而 fixture 的 trap 只杀进程不改配置
  15. ⚠️ **member 在这个屏幕上结构性到不了写路径**（2026-08-18 实测结论,
      同日 LAN+TLS 复验仍成立: member 屏上 测试连接/删除提供商/保存 各 0 个、
      provider 行 0 个 —— 不是没跑）: providers.list 与 providers.catalog 都被 ADMIN_PREFIXES 拒掉,
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
      条数以 fixture 自己的输出为准——散文里抄一个数，它第一次就漂了
      （这里写 16，屏幕上是 18）。它断言：feishu / line / qq 各自被工厂构造；msteams 作为对照组
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
  logs:     $QA_ROOT/server.log · $QA_ROOT/mock.log · \$ALEPH_HOME/logs/
  item 9/12/13 的落盘断言:  grep -A6 '^\[providers\.' $CONFIG
  item 12 的拨号证据:       cat $QA_ROOT/probes.jsonl   (一行一次探针, 带 model)
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
