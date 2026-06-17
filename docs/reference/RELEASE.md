# 发版流程 (Release Process)

> 由根 `CLAUDE.md` 的「发版流程」一行指针指向本文。

**由 AI (Claude) 驱动的两步流程：**

1. **AI 写版本日志** — 读取**上一个 release 版本到 HEAD 之间**的 git log（通过 `git log <上次release commit>..HEAD`），总结 10-20 条有价值的内容，分为 Added（新增功能）和 Fixed（修复）两个分类，写入 CHANGELOG.md
2. **运行 `just release YY.M.D`** — 自动完成：版本号更新 + 提交推送 + 触发**三产物 × 三平台**构建并发布（完整桌面 App 内置 `aleph-server` / Aleph Panel 纯壳 App / 独立 `aleph-server` 二进制 + `install.sh`，同一 GitHub Release）

`just release` 会校验 CHANGELOG.md 中是否有对应版本的条目，没有则拒绝发布。GitHub Release 页面自动从 CHANGELOG.md 提取版本日志。

> **可选预检**：发布前可先跑 `just verify-build`，以 build-only 模式在 CI 上构建**三产物 × 三平台**（完整 App / Panel 纯壳 App / 独立 server，只构建 + 传 artifacts，不打 tag、不发布），确认都能正常构建后再 `just release`。同一 workflow（`aleph-app-release.yml`）的 `publish` 输入：`off`=纯验证，`on`=`just release` 走的发布模式。

> **监控 CI（每次发版复用）**：用 `python3 scripts/poll_release_run.py`（不带参数自动选最新 run）做 **fail-fast 轮询**——job 级检查，任一平台 job 失败/取消立即退出并打印是哪个平台，而不是像 `gh run watch --exit-status` 那样整轮（~33 分钟）才返回、漏报单平台早败。脚本内部循环、守 gh 空响应/超时（开发网络有过 DNS/代理瞬断），budget（默认 540s）用尽后打印 `RESULT=STILL_RUNNING`，直接再跑一次即可。最后一行 `RESULT=COMPLETED conclusion=success` 全绿 / `RESULT=FAILED` 附失败 job 名。**平台门控陷阱**：CI 失败常是 `#[cfg(target_os)]` 分支里的 `const fn` 调运行时函数——本地 macOS 不编译该分支故看不见，失败后一次性 grep `desktop/{linux,windows,shell,shared}` 全部 const fn 亲读每个 body。
