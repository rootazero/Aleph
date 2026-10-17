//! Independent Task9 guards. Source AST evidence and package graph evidence
//! deliberately have different inputs and derivations.

use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/modularization/imports")
        .join(name)
}

#[test]
fn actual_api_and_allowed_fixture_have_nonempty_ast_inventories() {
    let api =
        scan_contract_imports(&Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/aleph-slot-api"))
            .unwrap();
    assert!(api.files > 0 && api.items > 0 && api.paths > 0, "{api:?}");
    let allowed = scan_contract_imports(&fixture("allowed")).unwrap();
    assert!(allowed.files > 0 && allowed.items > 0 && allowed.paths > 0);
}

#[test]
fn each_forbidden_import_form_has_path_and_location_evidence() {
    for (case, spelling) in [
        ("plain", "alephcore::providers::AiProvider"),
        ("braced", "alephcore::providers::AiProvider"),
        ("multiline", "alephcore::providers::AiProvider"),
        ("alias", "c::error::AlephError"),
        ("alias_use", "p::AiProvider"),
        ("glob", "alephcore::*"),
        ("qualified", "alephcore::providers::ProviderResponse"),
        (
            "leading_qualified",
            "::alephcore::providers::ProviderResponse",
        ),
        ("reexport", "alephcore::providers::ProviderResponse"),
        ("extern_crate", "c::providers::*"),
        ("macro_path", "alephcore::forbidden"),
        ("alias_macro", "c::forbidden"),
    ] {
        let root = fixture(case);
        let error = scan_contract_imports(&root).unwrap_err();
        assert!(error.contains(spelling), "{case}: {error}");
        assert!(error.contains("alephcore"), "{error}");
        assert!(error.contains("src/lib.rs:"), "{error}");
        assert!(error.contains(&root.display().to_string()), "{error}");
    }
}

#[test]
fn imports_fail_closed_on_absent_empty_bad_utf8_and_parse_errors() {
    for case in ["parse_error", "empty", "does_not_exist"] {
        let root = fixture(case);
        let error = scan_contract_imports(&root).unwrap_err();
        assert!(error.contains(&root.display().to_string()), "{error}");
    }
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("src")).unwrap();
    std::fs::write(temp.path().join("src/lib.rs"), [0xff, 0xfe]).unwrap();
    assert!(scan_contract_imports(temp.path())
        .unwrap_err()
        .contains("lib.rs"));
}

#[test]
fn ast_alias_fixpoint_is_local_to_each_scan() {
    let allowed = fixture("allowed");
    let bad = fixture("alias_use");
    let clean = scan_contract_imports(&allowed).unwrap();
    assert!(scan_contract_imports(&bad).is_err());
    assert_eq!(clean, scan_contract_imports(&allowed).unwrap());
    assert!(scan_contract_imports(&bad).is_err());
    assert_eq!(clean, scan_contract_imports(&allowed).unwrap());
}

#[test]
fn manifest_and_resolved_package_ids_have_only_approved_normal_edges() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    // Main supplies fresh, FULL guarded `cargo metadata --format-version 1`.
    // Never shell out to Cargo from an integration test or accept no-deps data.
    let path = std::env::var_os("ALEPH_TASK9_METADATA_JSON")
        .expect("set ALEPH_TASK9_METADATA_JSON to Main's fresh guarded full metadata JSON");
    let raw = std::fs::read_to_string(&path).expect("read guarded metadata JSON");
    let manifest_path = root.join("crates/aleph-slot-api/Cargo.toml");
    let manifest = std::fs::read_to_string(&manifest_path).unwrap();
    let evidence = check_dependency_evidence(&raw, &manifest, &manifest_path).unwrap();
    assert_eq!(evidence.normal.len(), APPROVED_NORMAL.len());
    assert!(!evidence.dev.is_empty());
    assert!(evidence.build.is_empty());
    println!("Task9 actual API direct package edges: {evidence:?}");
}

#[test]
fn dependency_guard_resolves_renamed_packages_and_distinguishes_edge_kinds() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = root.join("crates/aleph-slot-api/Cargo.toml");
    let manifest = std::fs::read_to_string(&path).unwrap();
    let mut graph = dependency_fixture(&path);
    let raw = serde_json::to_string(&graph).unwrap();
    assert!(check_dependency_evidence(&raw, &manifest, &path).is_ok());
    // An innocuous alias may not hide Core's actual package ID/name.
    let deps = graph["resolve"]["nodes"][0]["deps"].as_array_mut().unwrap();
    deps.push(serde_json::json!({"name":"harmless", "pkg":"core-id", "dep_kinds":[{"kind":null,"target":null}]}));
    graph["packages"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"id":"core-id","name":"alephcore"}));
    assert!(
        check_dependency_evidence(&graph.to_string(), &manifest, &path)
            .unwrap_err()
            .contains("alephcore")
    );
    graph["resolve"]["nodes"][0]["deps"]
        .as_array_mut()
        .unwrap()
        .pop();
    // Dev/build Core edges do not masquerade as normal package evidence.
    graph["resolve"]["nodes"][0]["deps"].as_array_mut().unwrap().push(
        serde_json::json!({"name":"core_dev", "pkg":"core-id", "dep_kinds":[{"kind":"dev","target":null},{"kind":"build","target":null}]}));
    let evidence = check_dependency_evidence(&graph.to_string(), &manifest, &path).unwrap();
    assert!(evidence.dev.iter().any(|e| e.contains("alephcore")));
    assert!(evidence.build.iter().any(|e| e.contains("alephcore")));
    assert!(!evidence.normal.iter().any(|e| e.contains("alephcore")));
}

#[test]
fn dependency_guard_rejects_missing_graph_ids_kinds_and_unapproved_libraries() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/aleph-slot-api/Cargo.toml");
    let manifest = std::fs::read_to_string(&path).unwrap();
    let clean = dependency_fixture(&path);
    for key in ["resolve", "packages"] {
        let mut bad = clean.clone();
        bad.as_object_mut().unwrap().remove(key);
        assert!(check_dependency_evidence(&bad.to_string(), &manifest, &path).is_err());
    }
    for mutation in [
        "empty",
        "missing_id",
        "missing_kinds",
        "unknown_kind",
        "unexpected",
        "renamed_driver",
    ] {
        let mut bad = clean.clone();
        match mutation {
            "empty" => bad["resolve"]["nodes"][0]["deps"] = serde_json::json!([]),
            "missing_id" => {
                bad["resolve"]["nodes"][0]["deps"][0]["pkg"] = "missing-package-id".into()
            }
            "missing_kinds" => {
                bad["resolve"]["nodes"][0]["deps"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("dep_kinds");
            }
            "unknown_kind" => {
                bad["resolve"]["nodes"][0]["deps"][0]["dep_kinds"][0]["kind"] = "mystery".into()
            }
            name => {
                let package = if name == "renamed_driver" {
                    "aleph-cdp"
                } else {
                    "unexpected-library"
                };
                bad["packages"]
                    .as_array_mut()
                    .unwrap()
                    .push(serde_json::json!({"id":"extra-id","name":package}));
                bad["resolve"]["nodes"][0]["deps"].as_array_mut().unwrap().push(serde_json::json!({"name":"innocent_alias","pkg":"extra-id","dep_kinds":[{"kind":null,"target":null}]}));
            }
        }
        assert!(
            check_dependency_evidence(&bad.to_string(), &manifest, &path).is_err(),
            "{mutation}"
        );
    }
    let renamed = manifest.replace(
        "url = \"2.5\"",
        "friendly = { package = \"alephcore\", version = \"1\" }",
    );
    assert!(
        check_dependency_evidence(&clean.to_string(), &renamed, &path)
            .unwrap_err()
            .contains("alephcore")
    );
    assert!(check_dependency_evidence("{}", "", &path).is_err());
}

use std::collections::{BTreeMap, BTreeSet};
use syn::spanned::Spanned;
use syn::visit::{self, Visit};

#[derive(Debug, Clone, PartialEq, Eq)]
struct ImportReport {
    files: usize,
    items: usize,
    paths: usize,
}

#[derive(Debug)]
struct SourcePath {
    spelling: String,
    segments: Vec<String>,
    line: usize,
    column: usize,
}

#[derive(Default)]
struct ImportVisitor {
    paths: Vec<SourcePath>,
    bindings: Vec<(String, String)>,
}

impl ImportVisitor {
    fn record(&mut self, segments: Vec<String>, leading: bool, span: proc_macro2::Span) {
        if segments.is_empty() {
            return;
        }
        let start = span.start();
        self.paths.push(SourcePath {
            spelling: format!("{}{}", if leading { "::" } else { "" }, segments.join("::")),
            segments,
            line: start.line,
            column: start.column + 1,
        });
    }

    fn use_tree(&mut self, tree: &syn::UseTree, prefix: &[String], leading: bool) {
        match tree {
            syn::UseTree::Path(path) => {
                let mut next = prefix.to_vec();
                next.push(path.ident.to_string());
                self.use_tree(&path.tree, &next, leading);
            }
            syn::UseTree::Group(group) => {
                for tree in &group.items {
                    self.use_tree(tree, prefix, leading);
                }
            }
            syn::UseTree::Glob(glob) => {
                let mut path = prefix.to_vec();
                path.push("*".into());
                self.record(path, leading, glob.span());
            }
            syn::UseTree::Name(name) => {
                let mut path = prefix.to_vec();
                let name = name.ident.to_string();
                if name != "self" {
                    path.push(name.clone());
                }
                if let Some(first) = path.first() {
                    let binding = if name == "self" {
                        path.last().unwrap().clone()
                    } else {
                        name
                    };
                    self.bindings.push((binding, first.clone()));
                }
                self.record(path, leading, tree.span());
            }
            syn::UseTree::Rename(rename) => {
                let mut path = prefix.to_vec();
                if rename.ident != "self" {
                    path.push(rename.ident.to_string());
                }
                if let Some(first) = path.first() {
                    self.bindings
                        .push((rename.rename.to_string(), first.clone()));
                }
                self.record(path, leading, rename.span());
            }
        }
    }

    fn violations(&self, file: &Path) -> Vec<String> {
        // Bounded source alias propagation, not compiler name resolution:
        // union of possible crate roots for use/extern bindings in ONE file.
        // Treat nested scopes conservatively; never let a later shadow hide Core.
        let mut aliases: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        loop {
            let mut changed = false;
            for (binding, first) in &self.bindings {
                let mut roots = aliases.get(first).cloned().unwrap_or_default();
                roots.insert(first.clone());
                let entry = aliases.entry(binding.clone()).or_default();
                let before = entry.len();
                entry.extend(roots);
                changed |= entry.len() != before;
            }
            if !changed {
                break;
            }
        }
        let mut violations = Vec::new();
        for path in &self.paths {
            let first = &path.segments[0];
            let mut roots = aliases.get(first).cloned().unwrap_or_default();
            roots.insert(first.clone());
            for root in roots.into_iter().filter(|name| forbidden_crate(name)) {
                violations.push(format!(
                    "{}:{}:{}: {} resolves to forbidden crate {root}",
                    file.display(),
                    path.line,
                    path.column,
                    path.spelling
                ));
            }
        }
        violations
    }
}

impl<'ast> Visit<'ast> for ImportVisitor {
    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        self.use_tree(&item.tree, &[], item.leading_colon.is_some());
        visit::visit_item_use(self, item);
    }

    fn visit_item_extern_crate(&mut self, item: &'ast syn::ItemExternCrate) {
        let source = item.ident.to_string();
        let binding = item
            .rename
            .as_ref()
            .map(|(_, name)| name.to_string())
            .unwrap_or_else(|| source.clone());
        self.bindings.push((binding, source.clone()));
        self.record(vec![source], false, item.span());
        visit::visit_item_extern_crate(self, item);
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        self.record(
            path.segments.iter().map(|s| s.ident.to_string()).collect(),
            path.leading_colon.is_some(),
            path.span(),
        );
        visit::visit_path(self, path);
    }
}

fn forbidden_crate(name: &str) -> bool {
    name == "alephcore"
        || name == "aleph_core"
        || name == "aleph_cdp"
        || name.starts_with("aleph_backend")
        || name.starts_with("aleph_driver")
        || name.starts_with("aleph_desktop")
        || name.starts_with("aleph_browser")
}

/// Parse all actual `.rs` sources under root/src, including cfg'd items.
/// Macro invocation PATHS are visited; unexpanded token payloads are opaque.
/// No code generation, Cargo metadata, producer lexer or fixture verdict table.
fn scan_contract_imports(root: &Path) -> Result<ImportReport, String> {
    fn walk(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
        for entry in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
            let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
            let path = entry.path();
            let kind = entry
                .file_type()
                .map_err(|e| format!("{}: {e}", path.display()))?;
            if kind.is_symlink() {
                return Err(format!("{}: symlink source", path.display()));
            }
            if kind.is_dir() {
                walk(&path, files)?;
            } else if kind.is_file() && path.extension().is_some_and(|x| x == "rs") {
                files.push(path);
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(&root.join("src"), &mut files)?;
    files.sort();
    let mut report = ImportReport {
        files: files.len(),
        items: 0,
        paths: 0,
    };
    let mut violations = Vec::new();
    for path in files {
        let raw = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let file = syn::parse_file(&raw).map_err(|e| {
            let start = e.span().start();
            format!(
                "{}:{}:{}: parse error: {e}",
                path.display(),
                start.line,
                start.column + 1
            )
        })?;
        report.items += file.items.len();
        let mut visitor = ImportVisitor::default();
        visitor.visit_file(&file);
        report.paths += visitor.paths.len();
        violations.extend(visitor.violations(&path));
    }
    if report.files == 0 || report.items == 0 || report.paths == 0 {
        return Err(format!(
            "{}: empty AST/import inventory {report:?}",
            root.display()
        ));
    }
    if !violations.is_empty() {
        return Err(format!(
            "{}: forbidden source imports:\n{}",
            root.display(),
            violations.join("\n")
        ));
    }
    Ok(report)
}

// Approved, explicit package names from accepted Tasks1–8. Never derive this
// allowlist from metadata or from the current manifest being checked.
const APPROVED_NORMAL: &[&str] = &[
    "aleph-protocol",
    "async-trait",
    "bytes",
    "chrono",
    "hex",
    "http",
    "schemars",
    "serde",
    "serde_json",
    "sha2",
    "thiserror",
    "tokio",
    "tracing",
    "url",
];

#[derive(Debug)]
struct DependencyEvidence {
    normal: Vec<String>,
    dev: Vec<String>,
    build: Vec<String>,
}

fn str_field<'a>(value: &'a serde_json::Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("missing/invalid metadata {key}"))
}

fn manifest_normal_packages(raw: &str) -> Result<BTreeSet<String>, String> {
    let manifest: toml::Value = toml::from_str(raw).map_err(|e| format!("manifest parse: {e}"))?;
    let table = manifest
        .get("dependencies")
        .and_then(|v| v.as_table())
        .ok_or("missing normal dependencies")?;
    fn add(
        table: &toml::map::Map<String, toml::Value>,
        names: &mut BTreeSet<String>,
    ) -> Result<(), String> {
        for (alias, spec) in table {
            let package = match spec {
                toml::Value::String(_) => alias.as_str(),
                toml::Value::Table(fields) => fields
                    .get("package")
                    .map(|v| v.as_str().ok_or("invalid package rename"))
                    .transpose()?
                    .unwrap_or(alias),
                _ => return Err(format!("invalid dependency {alias}")),
            };
            if !APPROVED_NORMAL.contains(&package) {
                return Err(format!(
                    "unapproved normal manifest package {package} (alias {alias})"
                ));
            }
            names.insert(package.to_string());
        }
        Ok(())
    }
    let mut names = BTreeSet::new();
    add(table, &mut names)?;
    if let Some(targets) = manifest.get("target") {
        for (_, target) in targets.as_table().ok_or("invalid target table")? {
            if let Some(deps) = target.get("dependencies") {
                add(
                    deps.as_table().ok_or("invalid target dependencies")?,
                    &mut names,
                )?;
            }
        }
    }
    let approved: BTreeSet<_> = APPROVED_NORMAL.iter().map(|s| s.to_string()).collect();
    if names != approved {
        return Err(format!(
            "normal manifest packages {names:?} differ from approved {approved:?}"
        ));
    }
    Ok(names)
}

/// Direct API edges only. `deps[].pkg` joins package IDs before inspecting
/// names; deps[].name is merely a Rust rename. Null kind means NORMAL, never
/// absent/safe. Dev/build edges are retained separately (no transitive claims).
fn check_dependency_evidence(
    raw: &str,
    manifest: &str,
    manifest_path: &Path,
) -> Result<DependencyEvidence, String> {
    let manifest_names = manifest_normal_packages(manifest)?;
    let graph: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("metadata parse: {e}"))?;
    let packages = graph
        .get("packages")
        .and_then(|v| v.as_array())
        .ok_or("missing packages")?;
    let mut by_id = BTreeMap::new();
    for package in packages {
        let id = str_field(package, "id")?;
        let name = str_field(package, "name")?;
        if by_id.insert(id, name).is_some() {
            return Err(format!("duplicate package ID {id}"));
        }
    }
    let apis: Vec<_> = packages
        .iter()
        .filter(|p| {
            p.get("name").and_then(|v| v.as_str()) == Some("aleph-slot-api")
                && p.get("manifest_path")
                    .and_then(|v| v.as_str())
                    .is_some_and(|p| Path::new(p) == manifest_path)
        })
        .collect();
    if apis.len() != 1 {
        return Err("expected one actual API package manifest/ID".into());
    }
    let api_id = str_field(apis[0], "id")?;
    let nodes = graph
        .get("resolve")
        .and_then(|r| r.get("nodes"))
        .and_then(|v| v.as_array())
        .ok_or("missing resolve.nodes; full metadata required")?;
    let owners: Vec<_> = nodes
        .iter()
        .filter(|n| n.get("id").and_then(|v| v.as_str()) == Some(api_id))
        .collect();
    if owners.len() != 1 {
        return Err(format!("missing/duplicate API resolve node {api_id}"));
    }
    let deps = owners[0]
        .get("deps")
        .and_then(|v| v.as_array())
        .ok_or("missing API deps")?;
    if deps.is_empty() {
        return Err("empty API package graph".into());
    }
    let mut result = DependencyEvidence {
        normal: Vec::new(),
        dev: Vec::new(),
        build: Vec::new(),
    };
    let mut normal_names = BTreeSet::new();
    for dep in deps {
        let id = str_field(dep, "pkg")?;
        let alias = str_field(dep, "name")?;
        let package = *by_id
            .get(id)
            .ok_or_else(|| format!("unresolved dependency package ID {id}"))?;
        let kinds = dep
            .get("dep_kinds")
            .and_then(|v| v.as_array())
            .filter(|v| !v.is_empty())
            .ok_or("missing/empty dep_kinds")?;
        for kind in kinds {
            let label = format!(
                "{alias} -> {package} [{id}] target={}",
                kind.get("target").unwrap_or(&serde_json::Value::Null)
            );
            match kind.get("kind") {
                Some(serde_json::Value::Null) => {
                    if !APPROVED_NORMAL.contains(&package) {
                        return Err(format!("unapproved normal package {label}"));
                    }
                    normal_names.insert(package.to_string());
                    result.normal.push(label);
                }
                Some(serde_json::Value::String(k)) if k == "dev" => result.dev.push(label),
                Some(serde_json::Value::String(k)) if k == "build" => result.build.push(label),
                _ => return Err(format!("missing/unknown dependency kind for {label}")),
            }
        }
    }
    if normal_names != manifest_names {
        return Err(format!(
            "resolved normal packages {normal_names:?} differ from manifest {manifest_names:?}"
        ));
    }
    result.normal.sort();
    result.dev.sort();
    result.build.sort();
    Ok(result)
}

fn dependency_fixture(manifest_path: &Path) -> serde_json::Value {
    let mut packages = vec![
        serde_json::json!({"id":"api-id", "name":"aleph-slot-api", "manifest_path":manifest_path}),
    ];
    let mut deps = Vec::new();
    for (index, name) in APPROVED_NORMAL.iter().enumerate() {
        let id = format!("approved-{index}");
        packages.push(serde_json::json!({"id":id,"name":name}));
        deps.push(serde_json::json!({"name":format!("renamed_{index}"),"pkg":id,"dep_kinds":[{"kind":null,"target":null}]}));
    }
    packages.push(serde_json::json!({"id":"syn-id","name":"syn"}));
    deps.push(
        serde_json::json!({"name":"syn","pkg":"syn-id","dep_kinds":[{"kind":"dev","target":null}]}),
    );
    serde_json::json!({"packages":packages,"resolve":{"nodes":[{"id":"api-id","deps":deps}]}})
}
