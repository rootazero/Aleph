# Fix review — E4

| ID | Verdict | Comment |
|---|---|---|
| E4-01 | FIXED | Lines 183–187 check `!parsers::is_path_inside(dir, &cursorrules_path)` before read; lines 206–210 check the rules directory; lines 218–224 check each rule file. All three symlink vectors blocked. |
| E4-02 | FIXED | Lines 869–877 reject actions with explicit `kind != "command"`, only legacy no-kind actions proceed to command registration. |
| E4-03 | FIXED | Lines 186–198 return error for unknown runtime before calling mapping functions; lines 273–277 validate explicit runtime against the three legal values before calling `runtime_to_kind`. |
| E4-04 | FIXED | Lines 1008–1012 return error for unknown scope instead of System fallback. |
| E4-05 | FIXED | Lines 301–304 choose `PluginKind::Mcp` and `cc_plugin_json::KIND_SIDECAR` entry when `.mcp.json` exists. |
| E4-06 | FIXED | Lines 827–833 parse raw JSON once, then branch on presence of `hooks` field to select wrapped or bare deserialization, accepting empty map in wrapped form. |

## New problems

none

## Batch verdict

Safe to commit. All six findings correctly fixed, no regressions introduced, guard placement and error messaging aligned with existing parsers.