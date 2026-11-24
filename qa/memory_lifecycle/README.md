# Memory lifecycle live probe / Memory 生命周期真机探针

Status: fixture added; live acceptance is pending the production wiring. / 状态：已加入测试装置，生产接线完成后再执行真机验收。

`probe.py` uses the existing WebSocket RPC client and local Anthropic stub. It copies the supplied server into its own artifact directory, verifies SHA-256 before starting, and creates a disposable HOME/ALEPH_HOME. It never builds a server or connects to an installed daemon. / 探针复用现有 WebSocket 客户端及本地 Anthropic stub，复制并校验指定服务二进制，使用独立 HOME/ALEPH_HOME；不自行构建，也不连接已安装的后台服务。

The scenario boots with Memory disabled, exercises status/start/stop through `tools.invoke`, creates a real note, checks explicit RPC/tool refusal while Off, completes real chat through the local stub, and observes the real raw Markdown archive after deleting only its own fixture sessions. It reactivates Memory, checks persistence, then holds a real `memory_reflect` model request behind a controllable local provider gate. After each signal it requires the process to remain alive beyond the former five-second exit, releases the actual request, and verifies completion. Both SIGTERM and a second boot with SIGINT are exercised. / 场景从 Memory 关闭状态启动，走生产工具完成状态查询、开启、关闭；创建真实笔记，验证 Off 时 RPC 和工具拒绝；通过本地模型 stub 完成聊天，并在删除本装置会话后验证真实 raw Markdown 归档；重新开启后验证持久化，将真实 `memory_reflect` 模型请求停在本地可控闸门；发送信号后要求进程存活超过原来的五秒强制退出时限，随后放行请求并验证完成。分别测试 SIGTERM 和重启后的 SIGINT。

Use the existing QA Python environment with `websockets` installed. / 使用已安装 `websockets` 的现有 QA Python 环境。

Before **every** Cargo command, run `python qa/browser_live/check_memory.py` and proceed only on `ALLOW` with at least 4 GiB available. Keep Cargo serial and set `CARGO_BUILD_JOBS=1`. / 每次 Cargo 命令之前运行内存检查，只有 `ALLOW` 且可用内存至少 4 GiB 才继续；Cargo 串行，`CARGO_BUILD_JOBS=1`。

After the gated build, copy/hash the fresh artifact before another Cargo command can replace it, and record the build command, feature provenance, source revision and SHA-256. Then run: / 受检构建完成后，在下一条 Cargo 命令之前复制并校验产物，记录构建命令、feature、源码版本及 SHA-256，然后运行：

```sh
python qa/memory_lifecycle/probe.py \
  --binary <dedicated-server-copy> \
  --sha256 <sha256> \
  --build-record <build-record.json>
```

Run the same command again with `--providerless` to verify management tools and raw chat through the actual providerless registry. This mode writes no provider configuration, starts no model stub, and checks the simulated assistant reply instead of `pong`. The controlled model-request hold applies only to the configured-provider run. / 再加 `--providerless` 执行一次，验证真正无 provider 时的同一 registry 管理工具和 raw 聊天；该模式不配置 provider、不启动模型 stub，检查模拟助手回复而非 `pong`；受控模型请求的持有证明仅适用于配置 provider 的运行。

The optional build record must contain the same `sha256`. Artifacts and `results.json` remain in the printed disposable directory. The probe only signals the exact server/stub subprocesses it creates. External readiness and exit guards fail the test; they are not an internal stop deadline. A forced kill cannot count as successful shutdown. / 可选构建记录必须含相同的 `sha256`。产物和 `results.json` 保留在输出的临时目录；探针只向自己创建的服务和 stub 进程发送信号。启动和退出的外部超时会使测试失败，不构成内部 stop 时限；强制终止不能算成功关闭。

This probe does not establish Linux/Windows native behavior, RSS reclamation, or external-effect exactly-once delivery. Integration tests and a fresh native Pi review remain separate acceptance evidence. / 本探针不证明 Linux/Windows 原生行为、RSS 回收或外部效果 exactly-once；集成测试及新的原生 Pi 审阅仍需单独验收。
