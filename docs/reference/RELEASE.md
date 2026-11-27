# 发版流程 (Release Process)

> 由根 `CLAUDE.md` 的「发版流程」一行指针指向本文。

## 桌面切换与回退 / Desktop cutover and rollback

最后一个 Tauri 完整桌面版是 [v26.9.1](https://github.com/rootazero/Aleph/releases/tag/v26.9.1)。保留对应平台的整包和同名 `.sig`；不要用新版本的 Panel 或 `aleph-server` 替换旧包内的文件。这里的签名是 Tauri 更新包签名，不代表安装包已通过操作系统签名或公证。 / Keep the platform archive and its matching `.sig` from the last full Tauri release. The signature authenticates the Tauri update asset, not its OS signing or notarization status.

| 平台 / Platform | v26.9.1 回退资产 / Rollback asset | SHA-256 |
| --- | --- | --- |
| macOS | `Aleph.app.tar.gz` | `7abe7e87a5ac484ac47bc72b1c5d1d5df8bc944827c134cd30612df89dd88e48` |
| Windows x64 | `Aleph_26.9.1_x64-setup.exe` | `75329d2c1d40a0e0b15028e78f70315df2aecd7fe5b8d8022d9d9a5f364d7d21` |
| Linux x64 | `Aleph_26.9.1_amd64.AppImage` | `8c7b94e15d29333aa7261b25c6f792c627cf8237f668c1afa1d7fcefae60b8d4` |

签名格式是 Base64 封装的 minisign 文件。先核对 SHA-256，再用 `v26.9.1` 标签的 `desktop/shell/tauri.conf.json` 中 `plugins.updater.pubkey` 验签：对 `.sig` 和 `pubkey` 分别做 Base64 解码，将输出存为 `<asset>.minisig` 与 `key.pub`，运行 `minisign -Vm <asset> -x <asset>.minisig -p key.pub`。 / Check the SHA-256 first. Base64-decode the matching `.sig` and the tagged `plugins.updater.pubkey` before verifying with minisign.

**切换到 NSH / Move to NSH:** 在 Tauri 中选择 **Quit & Stop Aleph**，确认旧本地 Gateway 已退出，再备份 Aleph 数据目录（默认 `~/.aleph`，包括配置和会话库）。手动安装新版 NSH 完整包。两版共用数据目录；已获准的回环连接不需要启动配对。旧版自动更新不会跨到 NSH。 / Quit and stop Tauri, back up Aleph data, then install the NSH desktop package manually. NSH reuses the data directory; an authorized loopback connection does not show startup pairing. The Tauri updater does not perform this migration.

**装回 Tauri / Reinstall Tauri:** 在 NSH 中选择 **Quit Aleph and Stop Server**，确认本地 Gateway 已退出。macOS 解开旧归档，用其中的 `Aleph.app` 替换 NSH 应用；Windows 用已验签的旧安装程序重装；Linux 给旧 AppImage 加执行权限后使用它。保留现有数据，不要把卸载当作清库步骤。如果旧版无法读取升级后的数据，先另存现状，再从切换前的备份恢复；不要直接覆盖唯一一份会话库。 / Quit NSH and stop its local Gateway before reinstalling the verified old package. Keep the current data. If the old version cannot read it, preserve a copy and restore the pre-cutover backup rather than overwriting the only session database.

**由 AI (Claude) 驱动的两步流程：**

1. **AI 写版本日志** — 读取**上一个 release 版本到 HEAD 之间**的 git log（通过 `git log <上次release commit>..HEAD`），总结 10-20 条有价值的内容，分为 Added（新增功能）和 Fixed（修复）两个分类，写入 CHANGELOG.md
2. **运行 `just release YY.M.D`** — 自动完成：版本号更新 + 提交推送 + 触发**三产物 × 三平台**构建并发布（完整桌面 App 内置 `aleph-server` / Aleph Panel 纯壳 App / 独立 `aleph-server` 二进制 + `install.sh`，同一 GitHub Release）

`just release` 会校验 CHANGELOG.md 中是否有对应版本的条目，没有则拒绝发布。GitHub Release 页面自动从 CHANGELOG.md 提取版本日志。

> **可选预检**：发布前可先跑 `just verify-build`，以 build-only 模式在 CI 上构建**三产物 × 三平台**（完整 App / Panel 纯壳 App / 独立 server，只构建 + 传 artifacts，不打 tag、不发布），确认都能正常构建后再 `just release`。同一 workflow（`aleph-app-release.yml`）的 `publish` 输入：`off`=纯验证，`on`=`just release` 走的发布模式。

> **监控 CI（每次发版复用）**：用 `python3 scripts/poll_release_run.py`（不带参数自动选最新 run）做 **fail-fast 轮询**——job 级检查，任一平台 job 失败/取消立即退出并打印是哪个平台，而不是像 `gh run watch --exit-status` 那样整轮（~33 分钟）才返回、漏报单平台早败。脚本内部循环、守 gh 空响应/超时（开发网络有过 DNS/代理瞬断），budget（默认 540s）用尽后打印 `RESULT=STILL_RUNNING`，直接再跑一次即可。最后一行 `RESULT=COMPLETED conclusion=success` 全绿 / `RESULT=FAILED` 附失败 job 名。**平台门控陷阱**：CI 失败常是 `#[cfg(target_os)]` 分支里的 `const fn` 调运行时函数——本地 macOS 不编译该分支故看不见，失败后一次性 grep `desktop/{linux,windows,shell,shared}` 全部 const fn 亲读每个 body。
