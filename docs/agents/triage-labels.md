# Triage Labels

The skills speak in terms of five canonical triage roles. This file maps those roles to the actual label strings used in this repo's issue tracker.

| Label in mattpocock/skills | Label in our tracker | Meaning                                  |
| -------------------------- | -------------------- | ---------------------------------------- |
| `needs-triage`             | `needs-triage`       | Maintainer needs to evaluate this issue  |
| `needs-info`               | `needs-info`         | Waiting on reporter for more information |
| `ready-for-agent`          | `ready-for-agent`    | Fully specified, ready for an AFK agent  |
| `ready-for-human`          | `ready-for-human`    | Requires human implementation            |
| `wontfix`                  | `wontfix`            | Will not be actioned                     |

When a skill mentions a role (e.g. "apply the AFK-ready triage label"), use the corresponding label string from this table.

Edit the right-hand column to match whatever vocabulary you actually use.

## 远端现状 (measured 2026-09-13, `rootazero/Aleph`)

`gh label list` 当时只有 `wontfix` 命中本表；另外四个**尚未在 GitHub 上创建**。

首次需要打某个标签时先建：

```bash
gh label create needs-triage    --description "Maintainer needs to evaluate"  --color d4c5f9
gh label create needs-info      --description "Waiting on reporter"           --color fbca04
gh label create ready-for-agent --description "Fully specified, AFK-ready"    --color 0e8a16
gh label create ready-for-human --description "Requires human implementation" --color 1d76db
```

⚠️ 别用现有的 `question` 顶替 `needs-info`——两者语义不同（一个是「我们在等你补信息」，一个是「这是个提问帖」），混用会让 triage 状态机读出错误的状态。

⚠️ 这一节是**测量结果**，带着它的 commit 一起读（判据 §18「量具会骗人」）：标签可能已被人手工创建或改名。打标签前用 `gh label list` 复核一次，别把这张表当成现状的真源。
