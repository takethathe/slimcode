//! Context files (AGENTS.md): discovery and system-prompt rendering.
//!
//! Mirrors pi's project-context loading: a global `AGENTS.md` at the slimcode
//! home dir (`<home>/AGENTS.md`, i.e. `$SLIMCODE_HOME` or `~/.slimcode`) plus
//! project `AGENTS.md` files from the working directory itself and from the
//! git repository root (the nearest directory holding a `.git` entry). The
//! scope of each file is labelled in the rendered prompt, and project
//! requirements are declared to override global ones.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::skills::escape_xml;

/// Where a context file was discovered from: the slimcode home dir (global)
/// or the working directory / git repository root (project).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextScope {
    /// `<home>/AGENTS.md` — applies to every project.
    Global,
    /// `AGENTS.md` in the cwd or the git repository root.
    Project,
}

/// A discovered `AGENTS.md`: its on-disk path, raw content, and scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextFile {
    pub path: PathBuf,
    pub content: String,
    pub scope: ContextScope,
}

/// Discover context files for a working directory: the global file at
/// `<home>/AGENTS.md` first, then the project files — the git repository root
/// (the nearest ancestor of `cwd` holding a `.git` entry, whether a directory
/// or a `gitdir:` file marker as used by worktrees/submodules) and then the
/// working directory itself. Project files are ordered root-first so the
/// nearest file lands last (highest precedence in the rendered prompt);
/// duplicate paths (e.g. a cwd nested under home, where the cwd lookup
/// re-finds the global file) are deduplicated in favour of the global scope.
pub fn load_context_files(home: &Path, cwd: &Path) -> Vec<ContextFile> {
    let mut files: Vec<ContextFile> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();

    // Global: `<home>/AGENTS.md` (the slimcode home dir, shared across
    // projects).
    let global = home.join("AGENTS.md");
    if global.is_file()
        && let Ok(content) = fs::read_to_string(&global)
    {
        files.push(ContextFile {
            path: global.clone(),
            content,
            scope: ContextScope::Global,
        });
        seen.insert(canonical_or_self(&global));
    }

    // Project locations, ordered root-first: the git root (if any) then the
    // cwd itself. A path already claimed (the global file, when cwd nests
    // under home) is skipped in favour of the earlier scope.
    let mut project_dirs: Vec<PathBuf> = Vec::new();
    if let Some(git_root) = find_git_root(cwd) {
        project_dirs.push(git_root);
    }
    project_dirs.push(cwd.to_path_buf());

    for dir in project_dirs {
        let file = dir.join("AGENTS.md");
        if !file.is_file() {
            continue;
        }
        if !seen.insert(canonical_or_self(&file)) {
            continue;
        }
        if let Ok(content) = fs::read_to_string(&file) {
            files.push(ContextFile {
                path: file,
                content,
                scope: ContextScope::Project,
            });
        }
    }

    files
}

/// Find the git repository root for `cwd`: the nearest ancestor directory
/// (including `cwd` itself) that holds a `.git` entry — either a directory
/// (a normal clone) or a file whose contents start with `gitdir:` (a linked
/// worktree or submodule). Returns `None` when no ancestor is a git repo.
fn find_git_root(cwd: &Path) -> Option<PathBuf> {
    let mut cur = Some(cwd);
    while let Some(dir) = cur {
        let git = dir.join(".git");
        if git.is_dir() || git.is_file() {
            return Some(dir.to_path_buf());
        }
        cur = dir.parent();
    }
    None
}

/// Canonicalize `p` for dedup, falling back to the raw path when the file
/// cannot be resolved (e.g. a broken symlink).
fn canonical_or_self(p: &Path) -> PathBuf {
    fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Render context files as a `## Project context` markdown section for the
/// system prompt: a markdown header (aligned with `## Skills` / `## Tools`)
/// followed by one `<project_instructions path scope>` XML block per file,
/// global first. Wrapping each file's content in an XML block keeps headings
/// or lists inside an AGENTS.md from clashing with the outer markdown
/// structure; the section closes by declaring that project requirements
/// override global ones. Returns "" when `files` is empty.
pub fn format_context_files(files: &[ContextFile]) -> String {
    if files.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "\n\n## Project context\n\n\
         Project-specific instructions and guidelines from AGENTS.md files; \
         project requirements override global requirements when they conflict.\n\n",
    );
    for file in files {
        let scope = match file.scope {
            ContextScope::Global => "global",
            ContextScope::Project => "project",
        };
        out.push_str(&format!(
            "<project_instructions path=\"{}\" scope=\"{}\">\n{}\n</project_instructions>\n\n",
            escape_xml(&file.path.display().to_string()),
            scope,
            file.content.trim(),
        ));
    }
    // Drop the trailing blank line so the following `## Skills` section sits
    // exactly one blank line after the last XML block.
    out.pop();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::unique_temp_dir;

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[test]
    fn loads_global_and_cwd_project_in_order() {
        let dir = unique_temp_dir("slimcode-context-basic");
        let home = dir.join("home");
        let cwd = dir.join("proj");
        write(&home.join("AGENTS.md"), "global rules\n");
        write(&cwd.join("AGENTS.md"), "project rules\n");

        let files = load_context_files(&home, &cwd);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, home.join("AGENTS.md"));
        assert_eq!(files[0].scope, ContextScope::Global);
        assert_eq!(files[0].content, "global rules\n");
        assert_eq!(files[1].path, cwd.join("AGENTS.md"));
        assert_eq!(files[1].scope, ContextScope::Project);
        assert_eq!(files[1].content, "project rules\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn loads_git_root_and_cwd_when_cwd_is_subdirectory() {
        let dir = unique_temp_dir("slimcode-context-git");
        let home = dir.join("home");
        let repo = dir.join("repo");
        let cwd = repo.join("src");
        write(&home.join("AGENTS.md"), "global\n");
        write(&repo.join(".git"), ""); // directory marker
        write(&repo.join("AGENTS.md"), "repo rules\n");
        write(&cwd.join("AGENTS.md"), "cwd rules\n");

        let files = load_context_files(&home, &cwd);
        let paths: Vec<&Path> = files.iter().map(|f| f.path.as_path()).collect();
        assert_eq!(
            paths,
            vec![
                home.join("AGENTS.md").as_path(),
                repo.join("AGENTS.md").as_path(),
                cwd.join("AGENTS.md").as_path(),
            ]
        );
        assert_eq!(files[0].scope, ContextScope::Global);
        assert_eq!(files[1].scope, ContextScope::Project);
        assert_eq!(files[2].scope, ContextScope::Project);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn git_root_recognized_via_git_file_marker() {
        // Worktrees/submodules use a `.git` *file* (gitdir: …) instead of a
        // directory; the root is still the dir holding the marker.
        let dir = unique_temp_dir("slimcode-context-gitfile");
        let home = dir.join("home");
        let repo = dir.join("repo");
        let cwd = repo.join("src");
        write(&repo.join(".git"), "gitdir: /elsewhere/repo.git\n");
        write(&repo.join("AGENTS.md"), "repo rules\n");

        let files = load_context_files(&home, &cwd);
        let paths: Vec<&Path> = files.iter().map(|f| f.path.as_path()).collect();
        assert_eq!(paths, vec![repo.join("AGENTS.md").as_path()]);
        assert_eq!(files[0].scope, ContextScope::Project);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ignores_ancestors_outside_cwd_and_git_root() {
        // Only the cwd and the git root (`.git` marker) qualify as project
        // locations; a plain ancestor AGENTS.md is NOT injected.
        let dir = unique_temp_dir("slimcode-context-ancestor");
        let home = dir.join("home");
        let repo = dir.join("repo");
        let cwd = repo.join("src");
        write(&home.join("AGENTS.md"), "global\n");
        write(&dir.join("AGENTS.md"), "loose ancestor, must NOT load\n");
        write(&repo.join(".git"), "");
        write(&repo.join("AGENTS.md"), "repo rules\n");
        write(&cwd.join("AGENTS.md"), "cwd rules\n");

        let files = load_context_files(&home, &cwd);
        let paths: Vec<&Path> = files.iter().map(|f| f.path.as_path()).collect();
        assert_eq!(
            paths,
            vec![
                home.join("AGENTS.md").as_path(),
                repo.join("AGENTS.md").as_path(),
                cwd.join("AGENTS.md").as_path(),
            ]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn git_root_deduped_when_cwd_is_git_root() {
        // When cwd is itself the git root, the AGENTS.md is loaded once.
        let dir = unique_temp_dir("slimcode-context-gitroot");
        let home = dir.join("home");
        let repo = dir.join("repo");
        write(&repo.join(".git"), "");
        write(&repo.join("AGENTS.md"), "repo rules\n");

        let files = load_context_files(&home, &repo);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, repo.join("AGENTS.md"));
        assert_eq!(files[0].scope, ContextScope::Project);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn dedupes_global_when_cwd_is_under_home() {
        let dir = unique_temp_dir("slimcode-context-dedupe");
        let home = dir.join("home");
        let cwd = home.join("proj");
        // Only the global file exists; the cwd lookup would re-find it.
        write(&home.join("AGENTS.md"), "global\n");

        let files = load_context_files(&home, &cwd);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].scope, ContextScope::Global);
        assert_eq!(files[0].path, home.join("AGENTS.md"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_when_no_agents_md_anywhere() {
        let dir = unique_temp_dir("slimcode-context-none");
        let home = dir.join("home");
        let cwd = dir.join("proj");
        fs::create_dir_all(&cwd).unwrap();
        assert!(load_context_files(&home, &cwd).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn renders_scopes_and_precedence_note() {
        let files = vec![
            ContextFile {
                path: PathBuf::from("/h/AGENTS.md"),
                content: "global rules\n".to_string(),
                scope: ContextScope::Global,
            },
            ContextFile {
                path: PathBuf::from("/p/AGENTS.md"),
                content: "project rules".to_string(),
                scope: ContextScope::Project,
            },
        ];
        // The section header is markdown (aligned with `## Skills` / `## Tools`)
        // while each file's content is wrapped in an XML block, so a `#` heading
        // or list inside an AGENTS.md cannot clash with the outer markdown.
        assert_eq!(
            format_context_files(&files),
            "\n\n## Project context\n\n\
             Project-specific instructions and guidelines from AGENTS.md files; \
             project requirements override global requirements when they conflict.\n\n\
             <project_instructions path=\"/h/AGENTS.md\" scope=\"global\">\n\
             global rules\n\
             </project_instructions>\n\n\
             <project_instructions path=\"/p/AGENTS.md\" scope=\"project\">\n\
             project rules\n\
             </project_instructions>\n"
        );
    }

    #[test]
    fn empty_input_renders_nothing() {
        assert_eq!(format_context_files(&[]), "");
    }

    #[test]
    fn escapes_path_in_scope_attribute() {
        let files = vec![ContextFile {
            path: PathBuf::from("/a&b/AGENTS.md"),
            content: "x".to_string(),
            scope: ContextScope::Project,
        }];
        let out = format_context_files(&files);
        assert!(out.contains("path=\"/a&amp;b/AGENTS.md\""), "got: {out}");
    }
}
