# 发版流程 (Release Process)

> 由根 `CLAUDE.md` 的「发版流程」一行指针指向本文。

## 桌面切换与回退 / Desktop cutover and rollback

最后一个 Tauri 完整桌面版是 [v26.9.1](https://github.com/rootazero/Aleph/releases/tag/v26.9.1)。保留对应平台的整包和同名 `.sig`；不要用新版本的 Panel 或 `aleph-server` 替换旧包内的文件。这里的签名是 Tauri 更新包签名，不代表安装包已通过操作系统签名或公证。 / Keep the platform archive and its matching `.sig` from the last full Tauri release. The signature authenticates the Tauri update asset, not its OS signing or notarization status.

| 平台 / Platform | v26.9.1 回退资产 / Rollback asset | SHA-256 |
| --- | --- | --- |
| macOS | `Aleph.app.tar.gz` | `7abe7e87a5ac484ac47bc72b1c5d1d5df8bc944827c134cd30612df89dd88e48` |
| Windows x64 | `Aleph_26.9.1_x64-setup.exe` | `75329d2c1d40a0e0b15028e78f70315df2aecd7fe5b8d8022d9d9a5f364d7d21` |
| Linux x64 | `Aleph_26.9.1_amd64.AppImage` | `8c7b94e15d29333aa7261b25c6f792c627cf8237f668c1afa1d7fcefae60b8d4` |

签名格式是 Base64 封装的 minisign 文件。先核对 SHA-256，再用 [v26.9.1 标签中的宿主配置](https://github.com/rootazero/Aleph/blob/v26.9.1/desktop/shell/tauri.conf.json) 中 `plugins.updater.pubkey` 验签：对 `.sig` 和 `pubkey` 分别做 Base64 解码，将输出存为 `<asset>.minisig` 与 `key.pub`，运行 `minisign -Vm <asset> -x <asset>.minisig -p key.pub`。 / Check the SHA-256 first. Base64-decode the matching `.sig` and the tagged `plugins.updater.pubkey` before verifying with minisign.

**切换到 NSH / Move to NSH:** 在 Tauri 中选择 **Quit & Stop Aleph**，确认旧本地 Gateway 已退出，再备份 Aleph 数据目录（默认 `~/.aleph`，包括配置和会话库）。手动安装新版 NSH 完整包。两版共用数据目录；已获准的回环连接不需要启动配对。旧版自动更新不会跨到 NSH。 / Quit and stop Tauri, back up Aleph data, then install the NSH desktop package manually. NSH reuses the data directory; an authorized loopback connection does not show startup pairing. The Tauri updater does not perform this migration.

**装回 Tauri / Reinstall Tauri:** 在 NSH 中选择 **Quit Aleph and Stop Server**，确认本地 Gateway 已退出。若 Gateway 由另一进程管理，按其启动方式停止服务，再替换应用。macOS 解开旧归档，用其中的 `Aleph.app` 替换 NSH 应用；Windows 用已验签的旧安装程序重装；Linux 给旧 AppImage 加执行权限后使用它。保留现有数据，不要把卸载当作清库步骤。如果旧版无法读取升级后的数据，先另存现状，再从切换前的备份恢复；不要直接覆盖唯一一份会话库。 / Quit NSH, stop any separately managed local Gateway through its service manager, and verify the local port has closed before reinstalling the verified old package. Keep the current data. If the old version cannot read it, preserve a copy and restore the pre-cutover backup rather than overwriting the only session database.

## 构建与发布 / Build and publish

1. 从上一个 release 到本次提交整理 `CHANGELOG.md`，并加入新 CalVer 小节 `## [YY.M.D]`。 / Add the release notes before bumping the version.
2. `just verify-build` 可在 `origin/main` 运行三平台构建，上传产物供验收。 / The build-only run uploads artifacts without publishing.
3. 从 `main` 运行 `just release YY.M.D`。命令核对版本与新标签，更新 `VERSION`、`Cargo.toml`、`Cargo.lock` 和 `desktop/nsh-shell/nsh.json`，提交后推送并触发同一个 workflow。 / Publishing requires a fresh tag and successful builds on all three platforms.

| 平台 / Platform | 桌面整包 / Desktop archive | 独立服务 / Standalone server |
| --- | --- | --- |
| macOS 14+ arm64 | `Aleph-<version>-macos-aarch64.tar.gz` | `aleph-server-aarch64-apple-darwin` |
| Windows x64 | `Aleph-<version>-windows-x86_64.zip` | `aleph-server-x86_64-pc-windows-msvc.exe` |
| Linux x64 | `Aleph-<version>-linux-x86_64.tar.gz` | `aleph-server-x86_64-unknown-linux-gnu` |

如 Windows 构建生成 `Aleph-setup.exe`，发布 job 会先安装到 runner 的临时目录，核对安装后的服务与同次构建的独立服务摘要。 / When Windows Setup is available, the release job installs it into a temporary directory on the runner and matches the installed server against the standalone server from the same build.

`aleph-app-release.yml` 使用固定 NSH 0.6.0。配置 `NSH_UPDATE_SECRET_KEY` 后，发布任务为最终归档生成 `aleph-update.json` 和 `latest.json.sig`。未配置密钥时发布手动下载包。 / The release also includes the standalone install scripts. Monitor platform jobs with `python3 scripts/poll_release_run.py`. [Build workflow](../../.github/workflows/aleph-app-release.yml)
