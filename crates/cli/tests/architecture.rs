//! The crate dependency matrix as a test (ADR-0011 D4, ticket 06).
//!
//! Every crate's `Cargo.toml` is read from disk — **all** dependency tables, so
//! a release, dev or build dependency counts — and the edges are asserted both
//! ways: a missing edge and an extra one both fail. The TUI is additionally
//! checked for the three application-layer names it must never mention.
//!
//! This is a tripwire, not a proof: it keeps the boundary `Cargo.toml` asserts
//! from eroding in review, and it fails loudly (with the file path) rather than
//! silently when a crate is renamed.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// The workspace root: this test lives in `crates/cli/tests`.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/cli lives under the workspace root")
        .to_path_buf()
}

/// The `slimcode-*` dependencies one manifest declares, from every dependency
/// table.
fn slimcode_deps(manifest: &Path) -> BTreeSet<String> {
    let text =
        fs::read_to_string(manifest).unwrap_or_else(|e| panic!("read {}: {e}", manifest.display()));
    let mut in_dependency_table = false;
    let mut found = BTreeSet::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            // `[dependencies]`, `[dev-dependencies]`, `[build-dependencies]`,
            // and any `[target.'…'.dependencies]` variant.
            in_dependency_table = line.ends_with("dependencies]");
            continue;
        }
        if !in_dependency_table {
            continue;
        }
        if let Some((key, _)) = line.split_once('=') {
            // Workspace-inherited keys read `slimcode-ai.workspace`; the crate
            // name is everything before the first dot.
            let key = key.trim().split('.').next().unwrap_or_default().trim();
            if key.starts_with("slimcode-") {
                found.insert(key.to_string());
            }
        }
    }
    found
}

/// Assert one crate declares exactly `expected` (missing and extra both fail).
fn assert_deps(crate_dir: &str, expected: &[&str]) {
    let manifest = workspace_root()
        .join("crates")
        .join(crate_dir)
        .join("Cargo.toml");
    let actual = slimcode_deps(&manifest);
    let expected: BTreeSet<String> = expected.iter().map(|s| s.to_string()).collect();
    assert_eq!(
        actual,
        expected,
        "{} drifted from the ADR-0011 D4 matrix",
        manifest.display()
    );
}

#[test]
fn ai_depends_on_no_other_crate() {
    assert_deps("ai", &[]);
}

#[test]
fn core_depends_on_ai_only() {
    assert_deps("core", &["slimcode-ai"]);
}

#[test]
fn app_depends_on_ai_core_and_commands() {
    assert_deps(
        "app",
        &["slimcode-ai", "slimcode-core", "slimcode-commands"],
    );
}

#[test]
fn commands_depends_on_no_other_crate() {
    assert_deps("commands", &[]);
}

#[test]
fn tui_depends_on_no_other_crate() {
    assert_deps("tui", &[]);
}

/// `cli_depends_on_every_crate`: the CLI takes every other crate (it is the
/// only binary), including the SDK it now consumes for the one-shot turn
/// orchestration (the cli-thin-shell spec: cli → {ai, core, app, commands,
/// tui, sdk}).
#[test]
fn cli_depends_on_every_crate() {
    assert_deps(
        "cli",
        &[
            "slimcode-ai",
            "slimcode-core",
            "slimcode-app",
            "slimcode-commands",
            "slimcode-tui",
            "slimcode-sdk",
        ],
    );
}

#[test]
fn sdk_depends_on_app_only() {
    assert_deps("sdk", &["slimcode-app"]);
}

/// Whether one manifest names `token` in any dependency table, ignoring comment
/// lines (prose may legitimately mention a forbidden name; the dependency
/// tables may not).
fn declares_in_dependency_tables(manifest: &Path, token: &str) -> bool {
    let text =
        fs::read_to_string(manifest).unwrap_or_else(|e| panic!("read {}: {e}", manifest.display()));
    let mut in_dependency_table = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_dependency_table = line.ends_with("dependencies]");
            continue;
        }
        if !in_dependency_table {
            continue;
        }
        // Drop a trailing comment before matching.
        let code = line.split('#').next().unwrap_or_default();
        if code.contains(token) {
            return true;
        }
    }
    false
}

/// `engine_layers_declare_no_tokio`: the async facade (ADR-0031 D3, engine
/// ticket 07) is runtime-agnostic, so slimcode's engine layers — `core`, `app`
/// and `sdk` — never name tokio as a dependency (the `futures` family is the
/// only async surface, and it carries no runtime). Checks dependency tables
/// only, so explanatory comments mentioning tokio stay legal.
#[test]
fn engine_layers_declare_no_tokio() {
    for crate_dir in ["core", "app", "sdk"] {
        let manifest = workspace_root()
            .join("crates")
            .join(crate_dir)
            .join("Cargo.toml");
        assert!(
            !declares_in_dependency_tables(&manifest, "tokio"),
            "{} must not depend on tokio (the async facade is runtime-agnostic)",
            manifest.display()
        );
    }
}

/// `ffi_depends_on_sdk_only`: the FFI module is a plain C ABI over the SDK and
/// nothing more (ADR-0023 D1 / the ffi spec D1), so it never reaches into
/// app/ai/core/commands/cli/tui — not even from a dev-dependency (this reader
/// covers every dependency table).
#[test]
fn ffi_depends_on_sdk_only() {
    assert_deps("ffi", &["slimcode-sdk"]);
}

/// The three application-layer names the TUI must not know (ADR-0011 D3 /
/// ticket 06): deliberately coarse. Application leakage beyond them stays a
/// review concern; the dependency matrix above is the real boundary.
#[test]
fn tui_source_names_no_application_type() {
    const FORBIDDEN: [&str; 3] = ["SessionStore", "SkillStore", "Config"];
    let root = workspace_root().join("crates/tui/src");
    let mut hits = Vec::new();
    for path in rust_sources(&root) {
        let text = fs::read_to_string(&path).expect("read TUI source");
        for name in FORBIDDEN {
            if text.contains(name) {
                hits.push(format!("{}: {name}", path.display()));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "the TUI source mentions application types: {hits:#?}"
    );
}

/// Every `.rs` file under `dir`, recursively (no walkdir dependency).
fn rust_sources(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            found.extend(rust_sources(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
    found
}
