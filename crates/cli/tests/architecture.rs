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

#[test]
fn cli_depends_on_every_other_crate() {
    assert_deps(
        "cli",
        &[
            "slimcode-ai",
            "slimcode-core",
            "slimcode-app",
            "slimcode-commands",
            "slimcode-tui",
        ],
    );
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
