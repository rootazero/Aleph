# Issue tracker: GitHub

Issues and PRDs for this repo live as GitHub issues on **`rootazero/Aleph`**. Use the `gh` CLI for all operations.

## Conventions

- **Create an issue**: `gh issue create --title "..." --body "..."`. Use a heredoc for multi-line bodies.
- **Read an issue**: `gh issue view <number> --comments`, filtering comments by `jq` and also fetching labels.
- **List issues**: `gh issue list --state open --json number,title,body,labels,comments --jq '[.[] | {number, title, body, labels: [.labels[].name], comments: [.comments[].body]}]'` with appropriate `--label` and `--state` filters.
- **Comment on an issue**: `gh issue comment <number> --body "..."`
- **Apply / remove labels**: `gh issue edit <number> --add-label "..."` / `--remove-label "..."`
- **Close**: `gh issue close <number> --comment "..."`

Infer the repo from `git remote -v` — `gh` does this automatically when run inside a clone.

## When a skill says "publish to the issue tracker"

Create a GitHub issue.

## When a skill says "fetch the relevant ticket"

Run `gh issue view <number> --comments`.

## 本仓库补充 (Aleph-specific)

- 开 issue 是**对外可见的写操作**。除非用户已明确授权本次批量开 issue，否则**先给草稿再执行**。
- Issue 正文中英皆可；**代码路径 / 判据编号保持原样**（例如 `src/harness/` · `FEATURE_LOCATOR §3.1` · `附录 E.0`）。
- 引用工程判据时写「**附录 E.N**」而不是「CLAUDE.md §N」——后者是已搬家的旧地址（判据全文现住 `docs/reference/FEATURE_LOCATOR.md` 附录 D/E）。
