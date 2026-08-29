//! File/process tool factories for the agent tool loop.
//!
//! Each factory captures a working directory and returns a [`Tool`] whose
//! `run` parses JSON arguments, performs the I/O, and returns text (or an
//! `Err` which becomes an `Error: …` tool message in history). These are the
//! read / write / bash / grep / find / ls tools from the v1 destination, plus
//! `edit_tool` which binds the pure `edit` engine (ticket 03) to disk.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::agent::Tool;
use crate::tools::edit::{Edit, apply_edits};

/// Cap on bytes returned by `bash_tool` (stdout + stderr combined).
const BASH_OUTPUT_CAP: usize = 16 * 1024;
/// Cap on matches returned by `grep_tool`.
const GREP_MATCH_CAP: usize = 200;
/// Cap on entries returned by `find_tool`.
const FIND_ENTRY_CAP: usize = 1000;

/// Resolve a tool-provided path (or ".") against the working directory.
fn resolve(cwd: &Path, path: Option<&str>) -> PathBuf {
    match path {
        Some(p) if !p.is_empty() => cwd.join(p),
        _ => cwd.to_path_buf(),
    }
}

/// Read a file and return its contents.
pub fn read_tool(cwd: impl Into<PathBuf>) -> Tool {
    let cwd = cwd.into();
    Tool::new(
        "read",
        "Read the full contents of a file at `path` (relative to the working directory).",
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "File path, relative to cwd"}
            },
            "required": ["path"]
        }),
        move |args| {
            let path = resolve(&cwd, args.get("path").and_then(Value::as_str));
            let meta = fs::metadata(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
            if meta.is_dir() {
                return Err(format!("read {}: is a directory", path.display()));
            }
            fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))
        },
    )
}

/// Write a file (creating parent directories), returning a confirmation.
pub fn write_tool(cwd: impl Into<PathBuf>) -> Tool {
    let cwd = cwd.into();
    Tool::new(
        "write",
        "Write `content` to the file at `path` (relative to the working directory), creating parent directories as needed.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "File path, relative to cwd"},
                "content": {"type": "string", "description": "Full file contents"}
            },
            "required": ["path", "content"]
        }),
        move |args| {
            let path = resolve(&cwd, args.get("path").and_then(Value::as_str));
            let content = args
                .get("content")
                .and_then(Value::as_str)
                .ok_or_else(|| "write: missing string `content`".to_string())?;
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|e| format!("write {}: {e}", path.display()))?;
            }
            fs::write(&path, content).map_err(|e| format!("write {}: {e}", path.display()))?;
            Ok(format!(
                "wrote {} bytes to {}",
                content.len(),
                path.display()
            ))
        },
    )
}

/// Run a shell command in the working directory, returning combined output.
pub fn bash_tool(cwd: impl Into<PathBuf>) -> Tool {
    let cwd = cwd.into();
    Tool::new(
        "bash",
        "Run a shell command in the working directory and return its combined stdout/stderr.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "Shell command to run"}
            },
            "required": ["command"]
        }),
        move |args| {
            let command = args
                .get("command")
                .and_then(Value::as_str)
                .ok_or_else(|| "bash: missing string `command`".to_string())?;
            let out = Command::new("sh")
                .arg("-c")
                .arg(command)
                .current_dir(&cwd)
                .output()
                .map_err(|e| format!("bash: failed to spawn: {e}"))?;
            let mut combined = String::from_utf8_lossy(&out.stdout).into_owned();
            combined.push_str(&String::from_utf8_lossy(&out.stderr));
            if combined.len() > BASH_OUTPUT_CAP {
                combined.truncate(BASH_OUTPUT_CAP);
                combined.push_str("\n...[output truncated]");
            }
            if out.status.success() {
                Ok(combined)
            } else {
                Err(format!(
                    "bash exited with {}:\n{}",
                    out.status
                        .code()
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "signal".to_string()),
                    combined
                ))
            }
        },
    )
}

/// Grep a file, or walk a directory grepping every file, returning matches as
/// `path:line: text` (capped).
pub fn grep_tool(cwd: impl Into<PathBuf>) -> Tool {
    let cwd = cwd.into();
    Tool::new(
        "grep",
        "Search for `pattern` (substring match) in the file at `path`, or recursively in `path` if it is a directory (defaults to the working directory). Returns `path:line: text` matches.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": "Substring to search for"},
                "path": {"type": "string", "description": "File or directory; defaults to cwd"}
            },
            "required": ["pattern"]
        }),
        move |args| {
            let pattern = args
                .get("pattern")
                .and_then(Value::as_str)
                .ok_or_else(|| "grep: missing string `pattern`".to_string())?;
            let root = resolve(&cwd, args.get("path").and_then(Value::as_str));
            let mut matches: Vec<String> = Vec::new();
            grep_into(&root, pattern, &mut matches, 0)?;
            if matches.is_empty() {
                return Ok(format!("no matches for {pattern:?} in {}", root.display()));
            }
            Ok(matches.join("\n"))
        },
    )
}

/// Collect grep matches from `path` (a file) or `dir` (recursively), filling
/// `out`. Depth caps recursion; match count caps output.
fn grep_into(
    path: &Path,
    pattern: &str,
    out: &mut Vec<String>,
    depth: usize,
) -> Result<(), String> {
    if out.len() >= GREP_MATCH_CAP {
        return Ok(());
    }
    if depth > 32 {
        return Ok(());
    }
    let meta = fs::metadata(path).map_err(|e| format!("grep {}: {e}", path.display()))?;
    if meta.is_dir() {
        for entry in fs::read_dir(path).map_err(|e| format!("grep {}: {e}", path.display()))? {
            let entry = entry.map_err(|e| format!("grep {}: {e}", path.display()))?;
            grep_into(&entry.path(), pattern, out, depth + 1)?;
        }
        return Ok(());
    }
    let content = fs::read_to_string(path).map_err(|e| format!("grep {}: {e}", path.display()))?;
    for (i, line) in content.lines().enumerate() {
        if out.len() >= GREP_MATCH_CAP {
            break;
        }
        if line.contains(pattern) {
            out.push(format!("{}:{}: {}", path.display(), i + 1, line));
        }
    }
    Ok(())
}

/// List files under a directory (recursively), optionally filtered by a
/// filename substring. Returns one path per line (capped).
pub fn find_tool(cwd: impl Into<PathBuf>) -> Tool {
    let cwd = cwd.into();
    Tool::new(
        "find",
        "List files under `path` (a directory; defaults to the working directory), recursively. `pattern` optionally filters by filename substring. Returns one path per line.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Directory; defaults to cwd"},
                "pattern": {"type": "string", "description": "Optional filename substring filter"}
            }
        }),
        move |args| {
            let root = resolve(&cwd, args.get("path").and_then(Value::as_str));
            let pattern = args.get("pattern").and_then(Value::as_str);
            let meta = fs::metadata(&root).map_err(|e| format!("find {}: {e}", root.display()))?;
            if !meta.is_dir() {
                return Err(format!("find {}: not a directory", root.display()));
            }
            let mut entries: Vec<String> = Vec::new();
            find_into(&root, pattern, &mut entries, 0)?;
            if entries.is_empty() {
                return Ok(format!("no files under {}", root.display()));
            }
            Ok(entries.join("\n"))
        },
    )
}

/// Collect file paths under `dir` into `out`, filtered by `pattern`.
fn find_into(
    dir: &Path,
    pattern: Option<&str>,
    out: &mut Vec<String>,
    depth: usize,
) -> Result<(), String> {
    if out.len() >= FIND_ENTRY_CAP || depth > 32 {
        return Ok(());
    }
    for entry in fs::read_dir(dir).map_err(|e| format!("find {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("find {}: {e}", dir.display()))?;
        let path = entry.path();
        let is_dir = entry
            .file_type()
            .map_err(|e| format!("find {}: {e}", path.display()))?
            .is_dir();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        let keep = match pattern {
            Some(p) => name.as_deref().is_some_and(|n| n.contains(p)),
            None => true,
        };
        if keep {
            out.push(path.display().to_string());
        }
        if is_dir {
            find_into(&path, pattern, out, depth + 1)?;
        }
    }
    Ok(())
}

/// List entries of a directory (one per line, sorted).
pub fn ls_tool(cwd: impl Into<PathBuf>) -> Tool {
    let cwd = cwd.into();
    Tool::new(
        "ls",
        "List the entries of `path` (a directory; defaults to the working directory), sorted, one per line.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Directory; defaults to cwd"}
            }
        }),
        move |args| {
            let dir = resolve(&cwd, args.get("path").and_then(Value::as_str));
            let meta = fs::metadata(&dir).map_err(|e| format!("ls {}: {e}", dir.display()))?;
            if !meta.is_dir() {
                return Err(format!("ls {}: not a directory", dir.display()));
            }
            let mut names: Vec<String> = fs::read_dir(&dir)
                .map_err(|e| format!("ls {}: {e}", dir.display()))?
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            Ok(names.join("\n"))
        },
    )
}

/// Bind the pure `edit` engine (ticket 03) to disk: reads the target file,
/// applies edits against its original content, and writes the result back.
pub fn edit_tool(cwd: impl Into<PathBuf>) -> Tool {
    let cwd = cwd.into();
    Tool::new(
        "edit",
        "Apply a set of edits to the file at `path` (relative to the working directory). Each edit matches the ORIGINAL file content; all edits must be unique and non-overlapping.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "File path, relative to cwd"},
                "edits": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "oldText": {"type": "string"},
                            "newText": {"type": "string"}
                        },
                        "required": ["oldText", "newText"]
                    }
                }
            },
            "required": ["path", "edits"]
        }),
        move |args| {
            let path = resolve(&cwd, args.get("path").and_then(Value::as_str));
            let edits: Vec<Edit> = args
                .get("edits")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|e| {
                            let old_text = e.get("oldText")?.as_str()?;
                            let new_text = e.get("newText")?.as_str()?;
                            Some(Edit::new(old_text.to_string(), new_text.to_string()))
                        })
                        .collect()
                })
                .ok_or_else(|| "edit: missing array `edits`".to_string())?;
            let content =
                fs::read_to_string(&path).map_err(|e| format!("edit {}: {e}", path.display()))?;
            let applied = apply_edits(&content, &edits, &path.display().to_string())?;
            fs::write(&path, &applied.new_content)
                .map_err(|e| format!("edit {}: {e}", path.display()))?;
            Ok(format!(
                "edited {} ({} block(s) replaced)\n{}",
                path.display(),
                applied.replaced_blocks,
                applied.diff
            ))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// Create a fresh temp working directory for a test.
    fn temp_cwd() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("slimcode-files-test-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn run(tool: &Tool, args: Value) -> Result<String, String> {
        (tool.run)(args)
    }

    #[test]
    fn read_returns_file_contents() {
        let cwd = temp_cwd();
        fs::write(cwd.join("a.txt"), "hello\nworld\n").unwrap();
        let t = read_tool(&cwd);
        let out = run(&t, serde_json::json!({"path": "a.txt"})).unwrap();
        assert_eq!(out, "hello\nworld\n");
    }

    #[test]
    fn read_missing_file_errors() {
        let cwd = temp_cwd();
        let t = read_tool(&cwd);
        let err = run(&t, serde_json::json!({"path": "nope.txt"})).unwrap_err();
        assert!(err.contains("nope.txt"), "err: {err}");
    }

    #[test]
    fn read_directory_errors() {
        let cwd = temp_cwd();
        let t = read_tool(&cwd);
        let err = run(&t, serde_json::json!({"path": "."})).unwrap_err();
        assert!(err.contains("directory"), "err: {err}");
    }

    #[test]
    fn write_creates_parent_dirs_and_content() {
        let cwd = temp_cwd();
        let t = write_tool(&cwd);
        let out = run(
            &t,
            serde_json::json!({"path": "sub/dir/b.txt", "content": "hi"}),
        )
        .unwrap();
        assert!(out.contains("2 bytes"), "out: {out}");
        assert_eq!(fs::read_to_string(cwd.join("sub/dir/b.txt")).unwrap(), "hi");
    }

    #[test]
    fn write_overwrites_existing() {
        let cwd = temp_cwd();
        fs::write(cwd.join("x.txt"), "old").unwrap();
        let t = write_tool(&cwd);
        run(&t, serde_json::json!({"path": "x.txt", "content": "new"})).unwrap();
        assert_eq!(fs::read_to_string(cwd.join("x.txt")).unwrap(), "new");
    }

    #[test]
    fn bash_runs_in_cwd_and_captures_stdout() {
        let cwd = temp_cwd();
        fs::write(cwd.join("greet.txt"), "hello").unwrap();
        let t = bash_tool(&cwd);
        let out = run(&t, serde_json::json!({"command": "cat greet.txt && pwd"})).unwrap();
        assert!(out.contains("hello"));
        assert!(out.contains(&cwd.display().to_string()), "out: {out}");
    }

    #[test]
    fn bash_stderr_joined_into_output() {
        let cwd = temp_cwd();
        let t = bash_tool(&cwd);
        let out = run(&t, serde_json::json!({"command": "echo out; echo err >&2"})).unwrap();
        assert!(out.contains("out"));
        assert!(out.contains("err"));
    }

    #[test]
    fn bash_nonzero_exit_is_error_with_output() {
        let cwd = temp_cwd();
        let t = bash_tool(&cwd);
        let err = run(&t, serde_json::json!({"command": "echo boom >&2; exit 3"})).unwrap_err();
        assert!(err.contains("3"), "err: {err}");
        assert!(err.contains("boom"));
    }

    #[test]
    fn grep_finds_matches_in_file_with_line_numbers() {
        let cwd = temp_cwd();
        fs::write(
            cwd.join("code.rs"),
            "fn main() {\n    let x = 1; // todo\n    println!(\"{x}\");\n}\n",
        )
        .unwrap();
        let t = grep_tool(&cwd);
        let out = run(
            &t,
            serde_json::json!({"pattern": "todo", "path": "code.rs"}),
        )
        .unwrap();
        assert!(out.contains("code.rs:2:"), "out: {out}");
    }

    #[test]
    fn grep_recurses_directory() {
        let cwd = temp_cwd();
        fs::create_dir_all(cwd.join("src")).unwrap();
        fs::write(cwd.join("src/a.rs"), "needle here\n").unwrap();
        fs::write(cwd.join("b.rs"), "nothing\n").unwrap();
        let t = grep_tool(&cwd);
        let out = run(&t, serde_json::json!({"pattern": "needle"})).unwrap();
        assert!(out.contains("a.rs"), "out: {out}");
        assert!(!out.contains("b.rs"), "out: {out}");
    }

    #[test]
    fn grep_no_matches_reports() {
        let cwd = temp_cwd();
        fs::write(cwd.join("c.txt"), "plain\n").unwrap();
        let t = grep_tool(&cwd);
        let out = run(&t, serde_json::json!({"pattern": "zzz", "path": "c.txt"})).unwrap();
        assert!(out.contains("no matches"), "out: {out}");
    }

    #[test]
    fn find_lists_files_recursively_and_filters() {
        let cwd = temp_cwd();
        fs::create_dir_all(cwd.join("src")).unwrap();
        fs::write(cwd.join("src/main.rs"), "x").unwrap();
        fs::write(cwd.join("README.md"), "x").unwrap();
        let t = find_tool(&cwd);
        let all = run(&t, serde_json::json!({})).unwrap();
        assert!(
            all.contains("main.rs") && all.contains("README.md"),
            "all: {all}"
        );
        let rs = run(&t, serde_json::json!({"pattern": ".rs"})).unwrap();
        assert!(rs.contains("main.rs"));
        assert!(!rs.contains("README.md"), "rs: {rs}");
    }

    #[test]
    fn ls_lists_sorted_entries() {
        let cwd = temp_cwd();
        fs::write(cwd.join("b.txt"), "").unwrap();
        fs::write(cwd.join("a.txt"), "").unwrap();
        fs::create_dir(cwd.join("dir")).unwrap();
        let t = ls_tool(&cwd);
        let out = run(&t, serde_json::json!({})).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines, vec!["a.txt", "b.txt", "dir"]);
    }

    #[test]
    fn edit_tool_applies_pi_style_edits_to_disk() {
        let cwd = temp_cwd();
        fs::write(cwd.join("f.txt"), "line1\nline2\nline3\n").unwrap();
        let t = edit_tool(&cwd);
        let out = run(
            &t,
            serde_json::json!({
                "path": "f.txt",
                "edits": [
                    {"oldText": "line1", "newText": "ONE"},
                    {"oldText": "line3", "newText": "THREE"}
                ]
            }),
        )
        .unwrap();
        assert!(out.contains("2 block(s)"), "out: {out}");
        assert_eq!(
            fs::read_to_string(cwd.join("f.txt")).unwrap(),
            "ONE\nline2\nTHREE\n"
        );
    }

    #[test]
    fn edit_tool_missing_target_errors() {
        let cwd = temp_cwd();
        let t = edit_tool(&cwd);
        let err = run(
            &t,
            serde_json::json!({
                "path": "ghost.txt",
                "edits": [{"oldText": "x", "newText": "y"}]
            }),
        )
        .unwrap_err();
        assert!(err.contains("ghost.txt"), "err: {err}");
    }
}
