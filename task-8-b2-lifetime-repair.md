# Task 8 B2 lifetime repair / 生命周期修复

Implemented the second source strategy after `#[serde(bound(deserialize = ""))]` failed compiler verification. 本轮仅完成源码修复，未进行编译或运行验证。

## Final implementation / 最终实现

- Source: `crates/aleph-slot-api/src/browser/routes.rs` (304 lines).
- `RouteRuleInfo` keeps `Clone`, `Debug`, `PartialEq`, `Serialize`, and all public field types, order, and visibility.
- Removed its derived `Deserialize`, `#[serde(bound(deserialize = ""))]`, and the field `deserialize_with` attribute.
- Added `impl<'de> serde::Deserialize<'de> for RouteRuleInfo` with no `'de: 'static` bound, a local `Field` enum, identifier visitor, and struct visitor.
- Entry: `deserialize_struct("RouteRuleInfo", FIELDS, RouteRuleInfoVisitor)`.
- Exact wire fields: `id`, `url_contains`, `method`, `kind_label`, `scope`, `hits`, `active`, `note`.
- Map parsing rejects duplicate known fields. Six required fields produce `missing_field` errors when absent; missing `method` and `note` default to `None`. Nested `Option<Option<String>>` slots detect duplicates even after explicit `null`.
- Unknown string or byte field identifiers map to `Ignore`; their values are consumed as `IgnoredAny`.
- `kind_label` is decoded into an owned `String`, matched against only `"mock"` and `"abort"`, and converted to static literals. Other labels return `unknown_variant`. The obsolete decoder helper was removed because equivalent decoding is inline; no dead helper remains.
- Sequence parsing reads the same eight fields in order, reports short sequences with `invalid_length`, and leaves trailing-element handling to the enclosing deserializer.
- No second DTO, new dependency, public String field, allocation leak, or change to `BrowserReply` / operation rows.

## Identity / 标识

- Repository HEAD: `fac11b6de66b7051f9aa102428170f4fd6c59cfe` (base commit, not a repair commit).
- Final routes.rs SHA-256: `259c8b80886353328064bfd20740091926370a7826caa38c71a68a7fef008cc0`.
- Both authorized paths are untracked; no staging or commit performed.

## Checks / 检查

Commands below used the repository root; only routes.rs was formatted or checked.

- `rustfmt --edition 2021 crates/aleph-slot-api/src/browser/routes.rs` — exit 0.
- `rustfmt --check --edition 2021 crates/aleph-slot-api/src/browser/routes.rs` — exit 0, no output; independently repeated on final source.
- `git diff --check -- crates/aleph-slot-api/src/browser/routes.rs` — exit 0, no diagnostics. Because the file is untracked, this command does not inspect its contents.
- `git diff --no-index --check /dev/null crates/aleph-slot-api/src/browser/routes.rs` — exit 1, no output or whitespace diagnostics; independently repeated on final source. Content differs from `/dev/null`.
- Final source SHA-256 and line count independently confirmed.

## Skips and boundary / 跳过与边界

No Cargo, rustc, tests, JSON runtime checks, staging, commit, install, or publication. No other repository files edited. Compilation and runtime behavior remain unverified; formatting and source review do not prove the original compilation failure is resolved. Work stops at this bounded repair and report.
