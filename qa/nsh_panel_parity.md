# NSH Panel parity reducer / NSH Panel 对齐归约器

`nsh_panel_parity.py` evaluates the approved debug-only parity measurement contract. It consumes value-only metric files from the Tauri baseline and the NSH candidate, removes the first 10 warmup samples, evaluates the next 30 samples, and writes a fixed JSON receipt.

`nsh_panel_parity.py` 负责批准的 debug-only 对齐测量协议。它读取 Tauri 基线与 NSH 候选的纯数值指标文件，丢弃前 10 次 warmup，使用接下来的 30 次测量，并写出固定结构的 JSON receipt。

## Input / 输入

Each input file has this shape:

```json
{
  "metrics": {
    "first_frame_ms": [40 numbers],
    "route_ms": [40 numbers],
    "input_ms": [40 numbers],
    "reconnect_ms": [40 numbers],
    "close_ms": [40 numbers],
    "rss_mb": [40 numbers],
    "threads": [40 numbers]
  }
}
```

The five latency metrics use median ≤ `1.10 ×` Tauri and p95 ≤ `1.20 ×` Tauri. RSS median uses a `1.10 ×` limit. Thread median uses an absolute `+2` limit. p95 is the one-based rank `ceil(0.95 × 30) = 29` after sorting the measured samples. The process set must exclude the external Gateway/daemon; the launcher owns only the two shell processes and its QA fixture.

五类延迟指标要求 median ≤ Tauri 的 `1.10 ×`、p95 ≤ Tauri 的 `1.20 ×`。RSS median 上限为 `1.10 ×`，线程 median 上限为绝对值 `+2`。p95 使用测量样本排序后的单基准第 `ceil(0.95 × 30) = 29` 位。进程集合必须排除外部 Gateway/daemon；launcher 只拥有两个 shell 进程与 QA fixture。

## Exit contract / 退出协议

- `0`: valid measurements and `threshold_status = ACCEPTED`.
- `2`: invalid input/measurement/receipt data.
- `3`: valid measurements but `threshold_status = NOT_ACCEPTED`.
- `4`: launcher or protocol failure.

- `0`：测量有效且 `threshold_status = ACCEPTED`。
- `2`：输入、测量或 receipt 数据无效。
- `3`：测量有效但 `threshold_status = NOT_ACCEPTED`。
- `4`：launcher 或协议失败。

Example:

```sh
python3 qa/nsh_panel_parity.py \
  --baseline /tmp/tauri-panel-metrics.json \
  --candidate /tmp/nsh-panel-metrics.json \
  --receipt /tmp/nsh-panel-parity-receipt.json \
  --run-id panel-parity-debug-001
```

The receipt records `baseline`, `candidate`, `comparisons`, `thresholds`, `threshold_status`, `acceptance_reason`, `invalid_reason`, and the fixed sampling protocol. A valid but rejected measurement is represented explicitly by `status: "VALID"` plus `threshold_status: "NOT_ACCEPTED"`; `invalid_reason` is reserved for invalid measurements.

receipt 会记录 `baseline`、`candidate`、`comparisons`、`thresholds`、`threshold_status`、`acceptance_reason`、`invalid_reason` 与固定采样协议。有效但未达阈值的测量必须明确表示为 `status: "VALID"` 与 `threshold_status: "NOT_ACCEPTED"`；`invalid_reason` 只用于无效测量。

The Tauri/NSH process launcher and app-level marker are a separate QA step. It must use an OS-temp direct child named `aleph-shell-qa-*`, a 32-byte lowercase-hex nonce, `<owned_root>/.qa-owned` containing `nonce + "\n"`, Python argv arrays with `shell=False`, and the debug-only `qa-owned-root` shell profile. This reducer does not claim that either native runtime has been launched.

Tauri/NSH 进程 launcher 与 app-level marker 属于独立 QA 步骤。它必须使用 OS temp 目录的直接子目录 `aleph-shell-qa-*`、32 字节小写十六进制 nonce、内容为 `nonce + "\n"` 的 `<owned_root>/.qa-owned`、`shell=False` 的 Python argv 数组，以及 debug-only `qa-owned-root` shell profile。本归约器不声称已经启动任何 native runtime。
