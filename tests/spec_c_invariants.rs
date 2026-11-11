//! Spec C cross-process-safety invariants, enforced against the source AST.
//!
//! `scripts/spec_c_regression.sh` used to hold these four invariants as text
//! greps. A grep cannot see the difference between production code and test
//! code, because both live in the same files: the gate reported the test
//! suite's own `Connection::open` calls as violations, and a comment in a
//! server-boot file satisfied the "every subcommand declares a policy" check.
//! The greps are gone; this module is the gate.
//!
//! Every `.rs` file under `src/` is parsed with `syn`, `#[cfg(test)]` regions
//! are skipped, and only what a release build would contain is inspected. A
//! `cfg` that *implies* `test` (`cfg(test)`, `cfg(all(test, unix))`) marks
//! test-only code; `cfg(not(test))` and `cfg(any(test, …))` do not, because
//! they can still compile into a release binary.
//!
//! The four invariants:
//!
//! 1. **SQLite opens** — a shared database is opened through
//!    `utils::sqlite_open::open_sqlite_safe` (WAL + `busy_timeout=5000` +
//!    `synchronous=NORMAL` + `foreign_keys=ON`). A raw
//!    `rusqlite::Connection::open` has no `busy_timeout`, so a concurrent
//!    writer gets `SQLITE_BUSY` instead of waiting.
//! 2. **Vault / ACP writes** — `secrets.vault` and `acp_sessions.json` are
//!    written through `vault_io` / `atomic_io::write_atomic` (an atomic
//!    rename), never through a raw `std::fs` write that can truncate a file a
//!    live daemon is using.
//! 3. **CLI lock policy** — every CLI subcommand module under
//!    `src/bin/aleph-server/commands/` declares its policy in a
//!    machine-readable `//! lock-policy: …` line, and the declaration agrees
//!    with the code: `with-policy` requires a `with_policy` / `try_with_policy`
//!    call, `run-no-lock` requires a `run_no_lock` call, `none` requires
//!    neither. A new subcommand without a declaration turns the gate red.
//! 4. **Singleton lock ownership** — the singleton lock file name lives only
//!    in `utils::instance_lock`, the acquisition helper is called only from the
//!    server boot and the CLI policy helper, and the legacy
//!    `acquire_instance_lock` shim stays deleted.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use proc_macro2::Span;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{
    Attribute, Expr, ExprCall, ExprLit, ExprPath, File, ImplItemFn, Item, ItemFn, ItemMod,
    ItemUse, Lit, Local, Meta, Pat, Path as SynPath, Token, UseTree,
};

/// The source tree this gate inspects.
const SRC_DIR: &str = "src";
/// The module list of the CLI subcommands — the source of truth for which
/// subcommand modules exist.
const COMMANDS_MOD: &str = "src/bin/aleph-server/commands/mod.rs";
/// The one file allowed to name the singleton lock file.
const INSTANCE_LOCK_REL: &str = "src/utils/instance_lock.rs";
/// The wrapper whose own `Connection::open` is the sanctioned one.
const SQLITE_OPEN_REL: &str = "src/utils/sqlite_open.rs";
/// The singleton lock file name. It belongs to `utils::instance_lock` alone.
const LOCK_FILE_NAME: &str = "aleph.lock";
/// The compat shim that used to be the second way to take the singleton lock.
/// It was deleted; it must not come back.
const LEGACY_LOCK_FN: &str = "acquire_instance_lock";
/// Files whose content must only ever be replaced atomically.
const PROTECTED_FILES: [&str; 2] = ["secrets.vault", "acp_sessions.json"];
/// The only two places allowed to acquire the singleton lock directly: the
/// server boot (`main`, before it forks) and the CLI policy helper (which
/// every policy-aware subcommand goes through).
const LOCK_ACQUIRE_OWNERS: [&str; 2] = ["src/bin/aleph-server/main.rs", "src/cli/policy.rs"];
/// The vocabulary of the `//! lock-policy:` declaration.
const POLICY_CLASSES: [&str; 3] = ["none", "with-policy", "run-no-lock"];

// ---------------------------------------------------------------------------
// Loading the sources
// ---------------------------------------------------------------------------

/// One `mod` declaration, as it appears in a file.
struct ModDecl {
    /// The module's name.
    ident: String,
    /// The paths the declaration can point at, in priority order.
    candidates: Vec<String>,
    /// Whether the declaration itself is test-only (`#[cfg(test)] mod foo;`).
    is_test_decl: bool,
}

/// What the gate needs to know about one file.
struct FileAnalysis {
    /// Path relative to the repository root, e.g. `src/cli/policy.rs`.
    rel: String,
    /// The violations found in this file's production code.
    findings: Vec<(Tag, String)>,
    /// The lock-policy API this file calls.
    policy_calls: BTreeSet<String>,
    /// The `mod` declarations this file makes.
    mods: Vec<ModDecl>,
    /// The module doc, with the `//!` markers stripped.
    doc: String,
}

/// The analysed source tree.
struct Tree {
    /// The analysis of every file, by relative path.
    by_rel: BTreeMap<String, FileAnalysis>,
    /// The files that only ever exist in a test build.
    test_only: BTreeSet<String>,
}

impl Tree {
    /// Does this file only exist in a test build?
    fn is_test_only(&self, rel: &str) -> bool {
        self.test_only.contains(rel)
    }

    /// The file a `mod` declaration points at, honouring `#[path = "…"]` and
    /// the `foo.rs` / `foo/mod.rs` fallback.
    fn resolve(&self, decl: &ModDecl) -> Option<&FileAnalysis> {
        decl.candidates
            .iter()
            .find_map(|candidate| self.by_rel.get(candidate))
    }

    /// The source file of one CLI subcommand.
    fn command_module(&self, name: &str) -> Option<&FileAnalysis> {
        let commands = self.by_rel.get(COMMANDS_MOD)?;
        let decl = commands.mods.iter().find(|m| m.ident == name)?;
        self.resolve(decl)
    }
}

/// The analysed tree, computed once per process.
///
/// The analysis holds plain data, so it can live in a `static`: the parsed
/// `syn::File` cannot (`Span` is not `Send` on stable), which is why the tree
/// is analysed here and only the results are cached.
fn tree() -> &'static Tree {
    static TREE: OnceLock<Tree> = OnceLock::new();
    TREE.get_or_init(|| analyse().expect("the source tree must parse"))
}

/// Could this file contain a violation of any of the four invariants?
///
/// The tokens the rules key on: a raw SQLite open, one of the protected file
/// names, the singleton lock file name, the singleton lock module, the
/// legacy lock helper, or the lock-policy API.
fn is_candidate(raw: &str) -> bool {
    raw.contains("Connection")
        || raw.contains("secrets.vault")
        || raw.contains("acp_sessions")
        || raw.contains(LOCK_FILE_NAME)
        || raw.contains("instance_lock")
        || raw.contains("try_acquire")
        || raw.contains(LEGACY_LOCK_FN)
        || raw.contains("run_no_lock")
        || raw.contains("with_policy")
}

/// Parse and inspect every file under `src/`.
fn analyse() -> Result<Tree, String> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut paths = Vec::new();
    collect_rs_files(&root.join(SRC_DIR), &mut paths);
    paths.sort();

    let mut by_rel: BTreeMap<String, FileAnalysis> = BTreeMap::new();
    for abs in &paths {
        let rel = rel_path(&root, abs);
        let raw = std::fs::read_to_string(abs)
            .map_err(|e| format!("{}: read error: {e}", abs.display()))?;
        let file = syn::parse_file(&raw).map_err(|e| {
            let start = e.span().start();
            format!(
                "{}:{}:{}: parse error: {e}",
                abs.display(),
                start.line,
                start.column + 1
            )
        })?;

        // The module declarations, for the module graph.
        let mods: Vec<ModDecl> = file
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Mod(m) if m.content.is_none() => Some(ModDecl {
                    ident: m.ident.to_string(),
                    candidates: mod_candidates(&rel, m),
                    is_test_decl: implies_test(&m.attrs),
                }),
                _ => None,
            })
            .collect();

        // Walking the AST of every file in `src/` costs minutes; the rules can
        // only fire on a handful of tokens, so the walk is restricted to the
        // files that contain one. The filter is a SUPERSET of what the rules
        // match: a false positive costs a walk, a false negative would hide a
        // violation.
        let mut scanner = Scanner::new(&rel);
        if is_candidate(&raw) {
            scanner.visit_file(&file);
        }
        let findings = std::mem::take(&mut scanner.findings);
        let policy_calls = std::mem::take(&mut scanner.policy_calls);
        drop(scanner);

        by_rel.insert(
            rel.clone(),
            FileAnalysis {
                rel,
                findings,
                policy_calls,
                mods,
                doc: module_doc(&file),
            },
        );
    }

    // Where each file is declared from, and whether that declaration is
    // test-only (`#[cfg(test)] mod foo;`).
    let mut declared_under_test: BTreeMap<String, bool> = BTreeMap::new();
    let mut declared_by: BTreeMap<String, String> = BTreeMap::new();
    for src in by_rel.values() {
        for decl in &src.mods {
            let Some(child) = decl.candidates.iter().find(|c| by_rel.contains_key(*c)) else {
                continue;
            };
            declared_under_test
                .entry(child.clone())
                .and_modify(|flag| *flag |= decl.is_test_decl)
                .or_insert(decl.is_test_decl);
            declared_by.insert(child.clone(), src.rel.clone());
        }
    }

    // Test-only status propagates down the module tree, so resolve it to a
    // fixed point.
    let mut test_only: BTreeSet<String> = BTreeSet::new();
    let rels: Vec<String> = by_rel.keys().cloned().collect();
    loop {
        let mut changed = false;
        for rel in &rels {
            if test_only.contains(rel) {
                continue;
            }
            let mut is_test = name_is_test_file(rel);
            if let Some(under) = declared_under_test.get(rel) {
                is_test |= *under;
            }
            if let Some(parent) = declared_by.get(rel) {
                is_test |= test_only.contains(parent);
            }
            if is_test {
                test_only.insert(rel.clone());
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    Ok(Tree { by_rel, test_only })
}

/// Collect `.rs` files, skipping symlinks (a symlinked file is not part of
/// the build the gate is about).
fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

fn rel_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// The directory a module's children live in.
///
/// Rust's rule: for `dir/name.rs` (a "directory module") the children live in
/// `dir/name/`; for `dir/mod.rs`, `dir/lib.rs` and `dir/main.rs` they live in
/// `dir/`. Getting this wrong makes a `#[cfg(test)] mod foo;` declaration
/// point at a file that does not exist, which would silently un-test it.
fn module_dir(declaring_rel: &str) -> String {
    let path = Path::new(declaring_rel);
    let parent = path
        .parent()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    match stem.as_str() {
        "mod" | "lib" | "main" => parent,
        _ => join_rel(&parent, &stem),
    }
}

/// The candidate paths a `mod foo;` declaration can point at, in priority
/// order.
fn mod_candidates(declaring_rel: &str, m: &ItemMod) -> Vec<String> {
    let parent_dir = module_dir(declaring_rel);
    if let Some(custom) = attr_path_value(&m.attrs) {
        return vec![join_rel(&parent_dir, &custom)];
    }
    let ident = m.ident.to_string();
    vec![
        join_rel(&parent_dir, &format!("{ident}.rs")),
        join_rel(&parent_dir, &format!("{ident}/mod.rs")),
    ]
}

fn join_rel(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

/// The value of a `#[path = "…"]` attribute.
fn attr_path_value(attrs: &[Attribute]) -> Option<String> {
    for attr in attrs {
        if attr.path().is_ident("path") {
            if let Meta::NameValue(nv) = &attr.meta {
                if let Expr::Lit(ExprLit {
                    lit: Lit::Str(s), ..
                }) = &nv.value
                {
                    return Some(s.value());
                }
            }
        }
    }
    None
}

/// Whether a file name or path marks it as test-only by convention.
fn name_is_test_file(rel: &str) -> bool {
    let path = Path::new(rel);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if name == "tests.rs" || name.ends_with("_tests.rs") || name.starts_with("test_") {
        return true;
    }
    path.components().any(|c| c.as_os_str() == "tests")
}

// ---------------------------------------------------------------------------
// `cfg` evaluation
// ---------------------------------------------------------------------------

/// Does this attribute list imply "test build"?
///
/// The question is not "is this compiled under `cargo test`" but "can this
/// code exist in a release build". `cfg(test)` cannot, so it implies test.
/// `cfg(all(test, unix))` cannot either — if `unix` is false the item does
/// not exist at all. `cfg(not(test))` and `cfg(any(test, feature = "x"))` can,
/// so they do not.
fn implies_test(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if attr.path().is_ident("test") {
            return true;
        }
        if attr.path().segments.len() == 2
            && attr.path().segments[0].ident == "tokio"
            && attr.path().segments[1].ident == "test"
        {
            return true;
        }
        attr.path().is_ident("cfg") && cfg_meta_implies_test(&attr.meta)
    })
}

fn cfg_meta_implies_test(meta: &Meta) -> bool {
    match meta {
        Meta::Path(p) => p.is_ident("test"),
        Meta::NameValue(_) => false,
        Meta::List(list) => {
            let name = list.path.get_ident().map(|i| i.to_string()).unwrap_or_default();
            match name.as_str() {
                // `cfg(a, b)` is `a AND b` — it implies test when one
                // predicate does.
                "cfg" => parse_meta_list(list)
                    .map(|metas| metas.iter().any(cfg_meta_implies_test))
                    .unwrap_or(false),
                // `all(a, b)` implies test when one conjunct does.
                "all" => parse_meta_list(list)
                    .map(|metas| metas.iter().any(cfg_meta_implies_test))
                    .unwrap_or(false),
                // `any(a, b)` implies test only when every arm does.
                "any" => parse_meta_list(list)
                    .map(|metas| !metas.is_empty() && metas.iter().all(cfg_meta_implies_test))
                    .unwrap_or(false),
                _ => false,
            }
        }
    }
}

fn parse_meta_list(list: &syn::MetaList) -> Option<Punctuated<Meta, Token![,]>> {
    list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
        .ok()
}

/// The attributes of an item, whichever variant it is.
fn item_attrs(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(i) => &i.attrs,
        Item::Enum(i) => &i.attrs,
        Item::ExternCrate(i) => &i.attrs,
        Item::Fn(i) => &i.attrs,
        Item::ForeignMod(i) => &i.attrs,
        Item::Impl(i) => &i.attrs,
        Item::Macro(i) => &i.attrs,
        Item::Mod(i) => &i.attrs,
        Item::Static(i) => &i.attrs,
        Item::Struct(i) => &i.attrs,
        Item::Trait(i) => &i.attrs,
        Item::TraitAlias(i) => &i.attrs,
        Item::Type(i) => &i.attrs,
        Item::Union(i) => &i.attrs,
        Item::Use(i) => &i.attrs,
        _ => &[],
    }
}

// ---------------------------------------------------------------------------
// The scanner
// ---------------------------------------------------------------------------

/// Which invariant a finding belongs to.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Tag {
    Sqlite,
    Vault,
    LockName,
    LockOwner,
    LegacyLock,
}

/// The string literals and identifiers an expression mentions.
#[derive(Default)]
struct Mentions {
    literals: Vec<String>,
    idents: BTreeSet<String>,
}

impl Mentions {
    /// Does the expression name one of the protected files?
    fn touches_protected(&self) -> bool {
        self.literals
            .iter()
            .any(|l| PROTECTED_FILES.iter().any(|p| l.contains(p)))
    }
}

impl<'ast> Visit<'ast> for Mentions {
    fn visit_expr_lit(&mut self, e: &'ast ExprLit) {
        if let Lit::Str(s) = &e.lit {
            self.literals.push(s.value());
        }
        visit::visit_expr_lit(self, e);
    }

    fn visit_expr_path(&mut self, p: &'ast ExprPath) {
        if let Some(last) = p.path.segments.last() {
            self.idents.insert(last.ident.to_string());
        }
        visit::visit_expr_path(self, p);
    }
}

/// Walks one file's production code and collects the violations.
struct Scanner<'a> {
    rel: &'a str,
    /// Local bindings whose value names a protected file.
    tainted: BTreeSet<String>,
    findings: Vec<(Tag, String)>,
    /// The lock-policy API this file calls.
    policy_calls: BTreeSet<String>,
}

impl<'a> Scanner<'a> {
    fn new(rel: &'a str) -> Self {
        Self {
            rel,
            tainted: BTreeSet::new(),
            findings: Vec::new(),
            policy_calls: BTreeSet::new(),
        }
    }

    fn report(&mut self, tag: Tag, span: Span, message: &str) {
        self.findings.push((
            tag,
            format!("{}:{}: {message}", self.rel, span.start().line),
        ));
    }
}

impl<'ast> Visit<'ast> for Scanner<'_> {
    /// Doc text is prose, not code: `#[doc = "… aleph.lock …"]` must never
    /// look like a literal in the source.
    fn visit_attribute(&mut self, attr: &'ast Attribute) {
        if attr.path().is_ident("doc") {
            return;
        }
        visit::visit_attribute(self, attr);
    }

    /// Skip test-only items — including whole inline `#[cfg(test)] mod`s.
    fn visit_item(&mut self, item: &'ast Item) {
        if implies_test(item_attrs(item)) {
            return;
        }
        visit::visit_item(self, item);
    }

    fn visit_item_fn(&mut self, f: &'ast ItemFn) {
        if implies_test(&f.attrs) {
            return;
        }
        let saved = std::mem::take(&mut self.tainted);
        visit::visit_item_fn(self, f);
        self.tainted = saved;
    }

    fn visit_impl_item_fn(&mut self, f: &'ast ImplItemFn) {
        if implies_test(&f.attrs) {
            return;
        }
        let saved = std::mem::take(&mut self.tainted);
        visit::visit_impl_item_fn(self, f);
        self.tainted = saved;
    }

    /// A local bound to a protected path stays protected.
    fn visit_local(&mut self, local: &'ast Local) {
        if let (Some(init), Pat::Ident(pat)) = (&local.init, &local.pat) {
            let mut collector = Mentions::default();
            collector.visit_expr(&init.expr);
            if collector.touches_protected()
                || collector.idents.iter().any(|i| self.tainted.contains(i))
            {
                self.tainted.insert(pat.ident.to_string());
            }
        }
        visit::visit_local(self, local);
    }

    fn visit_expr_lit(&mut self, e: &'ast ExprLit) {
        if let Lit::Str(s) = &e.lit {
            if s.value().contains(LOCK_FILE_NAME) && self.rel != INSTANCE_LOCK_REL {
                self.report(
                    Tag::LockName,
                    e.span(),
                    &format!(
                        "the singleton lock file name may only be written in {INSTANCE_LOCK_REL}"
                    ),
                );
            }
        }
        visit::visit_expr_lit(self, e);
    }

    fn visit_expr_path(&mut self, p: &'ast ExprPath) {
        if p.path.segments.iter().any(|s| s.ident == LEGACY_LOCK_FN) {
            self.report(
                Tag::LegacyLock,
                p.span(),
                &format!(
                    "`{LEGACY_LOCK_FN}` was deleted — the singleton lock is acquired through \
                     `utils::instance_lock::try_acquire` only"
                ),
            );
        }
        visit::visit_expr_path(self, p);
    }

    fn visit_item_use(&mut self, u: &'ast ItemUse) {
        if !LOCK_ACQUIRE_OWNERS.contains(&self.rel)
            && use_imports_lock_acquire(&u.tree)
        {
            self.report(
                Tag::LockOwner,
                u.span(),
                &format!(
                    "the singleton lock may only be acquired from {} — every other \
                     subcommand goes through `cli::policy`",
                    LOCK_ACQUIRE_OWNERS.join(" and ")
                ),
            );
        }
        visit::visit_item_use(self, u);
    }

    fn visit_expr_call(&mut self, call: &'ast ExprCall) {
        let callee: &SynPath = match &*call.func {
            Expr::Path(p) => &p.path,
            _ => {
                visit::visit_expr_call(self, call);
                return;
            }
        };
        let segments: Vec<String> = callee.segments.iter().map(|s| s.ident.to_string()).collect();
        let last = segments.last().map(String::as_str).unwrap_or("");

        // (1) A raw SQLite open.
        if segments.len() >= 2
            && segments[segments.len() - 2] == "Connection"
            && matches!(
                last,
                "open" | "open_with_flags" | "open_with_flags_and_vfs"
            )
            && self.rel != SQLITE_OPEN_REL
        {
            self.report(
                Tag::Sqlite,
                call.span(),
                &format!(
                    "`Connection::{last}` bypasses `utils::sqlite_open` — use \
                     `open_sqlite_safe` for a read-write database (WAL + `busy_timeout` + \
                     `foreign_keys`) and `open_sqlite_readonly` for a read-only one"
                ),
            );
        }

        // (2) A protected file written outside the atomic writer.
        if matches!(
            last,
            "write"
                | "create"
                | "remove_file"
                | "remove_dir"
                | "remove_dir_all"
                | "rename"
                | "copy"
                | "set_permissions"
        ) {
            let path_args: Vec<&Expr> = if matches!(last, "rename" | "copy") {
                call.args.iter().take(2).collect()
            } else {
                call.args.iter().take(1).collect()
            };
            for arg in path_args {
                let mut collector = Mentions::default();
                collector.visit_expr(arg);
                if collector.touches_protected()
                    || collector.idents.iter().any(|i| self.tainted.contains(i))
                {
                    self.report(
                        Tag::Vault,
                        call.span(),
                        &format!(
                            "`{last}` writes a protected file — use `vault_io` / \
                             `atomic_io::write_atomic` so a reader never sees a partial file"
                        ),
                    );
                    break;
                }
            }
        }

        // (3) The lock-policy API, for invariant (3).
        if matches!(last, "run_no_lock" | "with_policy" | "try_with_policy") {
            self.policy_calls.insert(last.to_string());
        }

        // (4) The singleton lock has exactly two owners.
        if last == "try_acquire"
            && segments.iter().any(|s| s == "instance_lock")
            && !LOCK_ACQUIRE_OWNERS.contains(&self.rel)
        {
            self.report(
                Tag::LockOwner,
                call.span(),
                &format!(
                    "the singleton lock may only be acquired from {} — every other \
                     subcommand goes through `cli::policy`",
                    LOCK_ACQUIRE_OWNERS.join(" and ")
                ),
            );
        }

        visit::visit_expr_call(self, call);
    }
}

/// Does this `use` tree import the singleton-lock acquisition helper?
fn use_imports_lock_acquire(tree: &UseTree) -> bool {
    fn walk(tree: &UseTree, inside_lock_module: bool) -> bool {
        match tree {
            UseTree::Path(p) => {
                let inside = inside_lock_module || p.ident == "instance_lock";
                walk(&p.tree, inside)
            }
            UseTree::Group(g) => g.items.iter().any(|t| walk(t, inside_lock_module)),
            UseTree::Name(n) => inside_lock_module && n.ident == "try_acquire",
            UseTree::Rename(r) => inside_lock_module && r.ident == "try_acquire",
            UseTree::Glob(_) => false,
        }
    }
    walk(tree, false)
}

/// The findings of one invariant, across the production tree.
fn findings_for(tree: &Tree, tag: Tag) -> Vec<String> {
    let mut findings = Vec::new();
    for src in tree.by_rel.values() {
        if tree.is_test_only(&src.rel) {
            continue;
        }
        findings.extend(
            src.findings
                .iter()
                .filter(|(t, _)| *t == tag)
                .map(|(_, message)| message.clone()),
        );
    }
    findings.sort();
    findings
}

// ---------------------------------------------------------------------------
// Invariant 3 — the CLI lock policy
// ---------------------------------------------------------------------------

/// The module doc of a file, with the `//!` markers stripped.
fn module_doc(file: &File) -> String {
    let mut doc = String::new();
    for attr in &file.attrs {
        if !attr.path().is_ident("doc") {
            continue;
        }
        if let Meta::NameValue(nv) = &attr.meta {
            if let Expr::Lit(ExprLit {
                lit: Lit::Str(s), ..
            }) = &nv.value
            {
                doc.push_str(&s.value());
                doc.push('\n');
            }
        }
    }
    doc
}

/// The classes named by a `//! lock-policy: …` declaration.
fn parse_lock_policy(doc: &str) -> Option<Vec<String>> {
    let line = doc.lines().find(|l| l.contains("lock-policy:"))?;
    let rest = line.split_once("lock-policy:")?.1;
    let classes: Vec<String> = rest
        .split(',')
        .filter_map(|part| part.split_whitespace().next().map(str::to_string))
        .filter(|c| !c.is_empty())
        .collect();
    if classes.is_empty() {
        None
    } else {
        Some(classes)
    }
}

/// Invariant 3: every CLI subcommand module declares its lock policy, and the
/// declaration agrees with the code.
fn check_lock_policy(tree: &Tree) -> Vec<String> {
    let mut findings = Vec::new();
    let Some(commands) = tree.by_rel.get(COMMANDS_MOD) else {
        findings.push(format!("{COMMANDS_MOD} is missing — no subcommand can be checked"));
        return findings;
    };

    let subcommands: Vec<String> = commands.mods.iter().map(|m| m.ident.clone()).collect();

    for name in subcommands {
        // `start` is the server boot: its lock is taken in `main()` before the
        // process forks, so it is not a CLI subcommand.
        if name == "start" {
            continue;
        }
        let Some(src) = tree.command_module(&name) else {
            findings.push(format!("{COMMANDS_MOD}: subcommand `{name}` has no source file"));
            continue;
        };

        let declared = match parse_lock_policy(&src.doc) {
            Some(classes) => classes,
            None => {
                findings.push(format!(
                    "{}: no `//! lock-policy: …` declaration — every CLI subcommand must \
                     declare one (none | with-policy | run-no-lock)",
                    src.rel
                ));
                continue;
            }
        };

        for class in &declared {
            if !POLICY_CLASSES.contains(&class.as_str()) {
                findings.push(format!(
                    "{}: unknown lock-policy class `{class}` (expected one of {})",
                    src.rel,
                    POLICY_CLASSES.join(", ")
                ));
            }
        }
        if declared.iter().any(|c| c == "none") && declared.len() > 1 {
            findings.push(format!(
                "{}: `none` cannot be combined with another class — a subcommand either \
                 takes the lock or it does not",
                src.rel
            ));
        }

        let calls = &src.policy_calls;

        if declared.iter().any(|c| c == "with-policy")
            && !calls.contains("with_policy")
            && !calls.contains("try_with_policy")
        {
            findings.push(format!(
                "{}: declares `with-policy` but never calls `with_policy` / `try_with_policy`",
                src.rel
            ));
        }
        if declared.iter().any(|c| c == "run-no-lock") && !calls.contains("run_no_lock") {
            findings.push(format!(
                "{}: declares `run-no-lock` but never calls `run_no_lock`",
                src.rel
            ));
        }
        if declared.iter().any(|c| c == "none") && !calls.is_empty() {
            findings.push(format!(
                "{}: declares `none` but calls the lock-policy API ({})",
                src.rel,
                calls.iter().cloned().collect::<Vec<_>>().join(", ")
            ));
        }
        // The agreement runs both ways: an *undeclared* policy call is as
        // wrong as a declared policy with no call behind it.
        if calls.contains("run_no_lock") && !declared.iter().any(|c| c == "run-no-lock") {
            findings.push(format!(
                "{}: calls `run_no_lock` but does not declare `run-no-lock`",
                src.rel
            ));
        }
        if (calls.contains("with_policy") || calls.contains("try_with_policy"))
            && !declared.iter().any(|c| c == "with-policy")
        {
            findings.push(format!(
                "{}: calls `with_policy` / `try_with_policy` but does not declare `with-policy`",
                src.rel
            ));
        }
    }

    findings.sort();
    findings
}

// ---------------------------------------------------------------------------
// The gate
// ---------------------------------------------------------------------------

#[test]
fn sqlite_opens_go_through_the_safe_helper() {
    let tree = tree();
    let findings = findings_for(tree, Tag::Sqlite);
    assert!(
        findings.is_empty(),
        "invariant 1 (SQLite opens) is violated:\n{}\n\nFix: open the database through \
         `alephcore::utils::sqlite_open::open_sqlite_safe` (read-write) or \
         `open_sqlite_readonly` (read-only).",
        findings.join("\n")
    );
}

#[test]
fn vault_and_acp_files_are_written_atomically() {
    let tree = tree();
    let findings = findings_for(tree, Tag::Vault);
    assert!(
        findings.is_empty(),
        "invariant 2 (vault / ACP writes) is violated:\n{}\n\nFix: write the file through \
         `utils::vault_io` or `utils::atomic_io::write_atomic`.",
        findings.join("\n")
    );
}

#[test]
fn every_cli_subcommand_declares_its_lock_policy() {
    let tree = tree();
    let findings = check_lock_policy(tree);
    assert!(
        findings.is_empty(),
        "invariant 3 (CLI lock policy) is violated:\n{}\n\nFix: declare the policy in the \
         module doc (`//! lock-policy: none | with-policy | run-no-lock`) and make the code \
         match the declaration.",
        findings.join("\n")
    );
}

#[test]
fn the_singleton_lock_has_exactly_one_owner() {
    let tree = tree();
    let mut findings = Vec::new();
    for tag in [Tag::LockName, Tag::LockOwner, Tag::LegacyLock] {
        findings.extend(findings_for(tree, tag));
    }
    findings.sort();
    findings.dedup();
    assert!(
        findings.is_empty(),
        "invariant 4 (singleton lock ownership) is violated:\n{}\n\nFix: take the lock through \
         `cli::policy`, or move the lock file name back into `utils::instance_lock`.",
        findings.join("\n")
    );
}
