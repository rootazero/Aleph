# NSH Memory Nebula verification / NSH 记忆星云验证

The opt-in TSX Panel uses one implementation across the macOS shell and Web v1. These runs use a temporary Gateway, pairing state, and Markdown corpus. See the [replay commands](../../../qa/nsh_s2_shell/README.md#memory-nebula-fixtures--记忆星云夹具) and [approved design](../specs/2026-11-27-nsh-memory-nebula-design.md).

本次使用临时 Gateway、配对状态和 Markdown 语料复跑，覆盖 macOS 原生 NSH 宿主和 Chrome Web v1。两端加载同一份 TSX 实现。

## Coverage / 覆盖范围

| Case / 用例 | Evidence / 证据 |
| --- | --- |
| Web with the normal fixture / Web 普通夹具 | Painted and picked a node after zoom, refreshed the same window with camera retained, distinguished identical paths in the organization and personal partitions, saved Markdown, renamed the note, and confirmed deletion. The oversized detail returned a single 92-byte error frame. / 真实画布完成缩放、拾取和保留视口的刷新，区分组织与个人分区的同路径笔记，保存 Markdown、改名、确认删除。超限详情返回一个 92 字节错误帧。 |
| Web with the dense fixture / Web 密集夹具 | Painted the exact 500-note neighborhood with 4,096 relations, including all 499 relations incident to the selected note. Core set `truncated: true` and Web v1 delivered the 484,348-byte response. / 精确聚焦绘制 500 节点与 4,096 条关系，保留所选笔记全部 499 条直接关系，Web v1 收到 484,348 字节响应与截断标记。 |
| Web with Memory off / Web 关闭记忆运行时 | The tab mounted, fired one graph request, and showed `memory_unavailable`. / 标签页挂载并发出一次图谱请求，显示 `memory_unavailable`。 |
| macOS with the normal fixture / macOS 普通夹具 | Native Desktop IPC drew the 100-of-101 overview and opened the exact personal note. / 原生 Desktop IPC 绘制 100/101 节点总览，并打开个人分区的精确笔记。 |
| macOS with the dense fixture / macOS 密集夹具 | Native Desktop IPC rendered the overview, opened the exact personal note, and displayed `500 notes in this neighborhood (limited)` after clicking Show neighborhood. The runner saved a native window image with the selected note and graph. / 原生 Desktop IPC 绘制总览、打开个人分区的精确笔记，并在点击聚焦后显示 500 节点与截断标记。窗口截图包含所选笔记与画布。 |
| macOS with Memory off / macOS 关闭记忆运行时 | The tab remained available and displayed `memory_unavailable`. / 标签页仍可打开并显示 `memory_unavailable`。 |
| Transport and authorization / 传输与授权 | Actual native client frames preserve structured RPC error data. Core tests cover bound peer and idempotency admission, payload reuse, cross-partition denial and capability advertisement. / 原生客户端帧保留结构化 RPC 错误数据。Core 测试覆盖已绑定会话、幂等参数、越分区拒绝和能力发布。 |

## Automated checks / 自动化检查

- `just panel-tsx-test` passed 112 NSH tests, the NSH type check, and 44 Web host tests. The viewport suite added 7 passing tests. / `just panel-tsx-test` 通过 112 项 NSH 测试、类型检查与 44 项 Web 宿主测试，另有 7 项视口测试通过。
- Final Core run passed 30 Gateway graph tests, 11 checked-note tests, 10 idempotency tests, 3 Capability projection tests, and the bound-peer mutation admission test. The checked-note suite includes resuming a failed rename after reopening the database. / 最终 Core 构建通过图谱 30 项、笔记校验写入 11 项、幂等 10 项、能力投影 3 项及已绑定会话写入测试。笔记写入覆盖重开数据库后继续重命名。
- Protocol passed 441 unit and 16 integration tests. Rust UI passed 2 scene, 9 shared layout, and 14 Native host tests. / 协议通过 441 项单元与 16 项集成测试。Rust UI 通过 2 项场景、9 项共享布局与 14 项原生宿主测试。

## macOS native measurement / macOS 原生测量

| Fixture / 夹具 | Overview observed / 总览观测 | Focus observed / 聚焦观测 | Sampled peak shell RSS / 宿主 RSS 峰值样本 |
| --- | ---: | ---: | ---: |
| Normal / 普通 | 940 ms | — | 158,448 KiB |
| Dense / 密集 | 926 ms | 3,066 ms | 180,128 KiB |
| Memory off / 停用 | — | — | 131,776 KiB |

Native observation includes capture, OCR and polling. RSS covers the whole shell process during the Nebula phase. The test used the existing PID-scoped click helper and a local RapidOCR CPU adapter after Apple Vision stalled. / 原生观测包含截图、OCR 与轮询，RSS 覆盖星云阶段的整个宿主进程。Apple Vision 超时后，验证通过现有按进程限定的点击工具和本机 RapidOCR CPU 适配器完成。

## Chrome measurement / Chrome 测量

| Build / 构建 | Nodes / 节点 | Edges / 边 | Drawable / 画布可见 | Zoom rAF p95 | Peak JS heap sample / JS 堆峰值样本 | RPC frame / RPC 帧 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Baseline `fe979a1d7` / 基线 | 100 | 100 | 54 ms | 8.6 ms | 19.3 MB | 28,539 B |
| NSH refactor, normal / 重构后普通 | 100 | 100 | 60 ms | 8.7 ms | 19.3 MB | 28,539 B |
| NSH refactor, dense focus / 重构后密集聚焦 | 500 | 4,096 | 313 ms | 9.1 ms | 18.2 MB | 484,348 B |

The baseline and normal runs use the same isolated fixture and 50 Chrome animation-frame samples across 12 zoom-in/out cycles. Time to drawable spans the initial tab or neighborhood action and the first visible Canvas pixel check. Dense focus uses 49 frame samples after indexing converged. Frame intervals include waiting for display refresh. Heap samples cover the JavaScript runtime and omit native and GPU allocations. These single runs record observed behavior on this machine. / 基线与普通夹具一致，12 组缩放共采集 50 个 Chrome 动画帧样本。首帧耗时从点击标签页或聚焦按钮到检测到绘制像素。密集聚焦在索引齐备后采集 49 帧。帧间隔含垂直同步等待，JavaScript 堆样本只覆盖 JS 运行时。数字记录本机单次运行的观测结果。
