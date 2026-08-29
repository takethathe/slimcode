//! Build the seven-tool set bound to a working directory.
//!
//! read / write / edit / bash / grep / find / ls — the v1 tool list from the
//! wayfinder destination. Each factory captures the cwd so the model operates
//! relative to the directory the CLI was launched in.

use std::path::Path;

use slimcode_agent::agent::Tool;
use slimcode_agent::tools::files;

/// The full v1 tool set for a working directory.
pub fn build_tools(cwd: &Path) -> Vec<Tool> {
    vec![
        files::read_tool(cwd),
        files::write_tool(cwd),
        files::edit_tool(cwd),
        files::bash_tool(cwd),
        files::grep_tool(cwd),
        files::find_tool(cwd),
        files::ls_tool(cwd),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn build_tools_exposes_the_seven_tools() {
        let cwd = std::env::temp_dir();
        let tools = build_tools(&cwd);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["read", "write", "edit", "bash", "grep", "find", "ls"]
        );
        assert!(tools.iter().all(|t| !t.description.is_empty()));
    }

    #[test]
    fn build_tools_tools_run_relative_to_cwd() {
        let dir = std::env::temp_dir().join(format!("slimcode-tools-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("hello.txt"), "world").unwrap();

        let tools = build_tools(&dir);
        let read = tools.iter().find(|t| t.name == "read").unwrap();
        let out = (read.run)(serde_json::json!({"path": "hello.txt"})).unwrap();
        assert_eq!(out, "world");
        let _ = fs::remove_dir_all(&dir);
    }
}
