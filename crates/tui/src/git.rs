//! Pure, unit-tested helpers for the two integration points that consult the
//! outside world from the terminal shell: the OSC 0 terminal title string and
//! the best-effort git branch name for the footer (ADR-0006 D5/D7). Both
//! return plain data; the shell only executes/exposes them.

use std::path::Path;
use std::process::Command;

/// The OSC 0 terminal title string: `slimcode - <session> - <cwd basename>`
/// (spec §Implementation Decisions "Terminal title", ADR-0006 D7). The cwd is
/// down to its basename so a long path does not flood the tab title; a cwd at
/// the filesystem root yields the root's name (e.g. `/` → `/` is skipped and
/// the path is used as-is when there is no basename).
pub fn terminal_title(session: &str, cwd: &Path) -> String {
    let base = cwd
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| cwd.display().to_string());
    format!("slimcode - {session} - {base}")
}

/// Best-effort current branch name via `git branch --show-current`, in `cwd`.
/// Returns `None` whenever the command fails or reports nothing (not a
/// repository, git absent, detached HEAD, or any other error) — the footer
/// simply omits the branch then (pi does the same best-effort lookup). Never
/// panics; the only failure mode is `None`.
pub fn current_branch(cwd: &Path) -> Option<String> {
    let out = Command::new("git")
        .arg("branch")
        .arg("--show-current")
        .current_dir(cwd)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let name = String::from_utf8(out.stdout).ok()?;
    let name = name.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_title_uses_cwd_basename() {
        assert_eq!(
            terminal_title("sess-1", Path::new("/home/u/proj")),
            "slimcode - sess-1 - proj"
        );
        assert_eq!(
            terminal_title("sess-1", Path::new("/home/u/proj/")),
            "slimcode - sess-1 - proj"
        );
        // Root: no basename → path as-is.
        assert_eq!(
            terminal_title("sess-1", Path::new("/")),
            "slimcode - sess-1 - /"
        );
        // Relative cwd without a file name falls back to the path itself.
        assert_eq!(
            terminal_title("sess-1", Path::new(".")),
            "slimcode - sess-1 - ."
        );
    }

    #[test]
    fn current_branch_none_outside_git_repo() {
        let outside = std::env::temp_dir().join(format!("slimcode-nogit-{}", std::process::id()));
        std::fs::create_dir_all(&outside).unwrap();
        // Not a repo → None (this holds even if a parent dir is a repo, since
        // we run `git -C <tmp>` which walks parents... guard by making the
        // temp dir itself unaffected: git refuses outside a work tree).
        let branch = current_branch(&outside);
        let _ = std::fs::remove_dir_all(&outside);
        assert!(branch.is_none(), "expected None, got {branch:?}");
    }

    #[test]
    fn current_branch_reads_checked_out_branch() {
        // git(1) presence is CI-dependent; skip silently when absent.
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("slimcode-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        run(&dir, &["init", "-b", "main"]);
        let branch = current_branch(&dir);
        run(&dir, &["branch", "-m", "feature/x"]);
        let renamed = current_branch(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(branch.as_deref(), Some("main"));
        assert_eq!(renamed.as_deref(), Some("feature/x"));
    }

    fn run(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git should run");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
