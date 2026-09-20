# diff-viewer plugin — CUT pending in Aleph-plugins (2026-09-06)

`plugins/diff-viewer` (Extism, `DiffSummaryInput`) has zero references in
Aleph outside the submodule; the Panel never called it and the server-side
`FileChange` presentation (this round) supersedes it. Remove it in the
Aleph-plugins repo, bump the submodule here, then re-run the grep above.
Consumers found in this repo by the grep: `./plugins-index.json`, `./src/hub/official_plugins.rs` (census entries only; remove both when the submodule is updated).
